/* pty_win32.c -- ConPTY-based PTY backend (Windows 10 1809+) */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <windows.h>

#include "pty.h"
#include "utils.h"

struct pty_impl_s {
    HPCON                       hpc;
    HANDLE                      hProcess;
    HANDLE                      hThread;
    HANDLE                      wait; /* RegisterWaitForSingleObject handle */
    PPROC_THREAD_ATTRIBUTE_LIST attr_list;
    char                       *in_pipe_name; /* leaked pipe name strings, freed at destroy */
    char                       *out_pipe_name;
};

static void async_free_cb(uv_handle_t *handle) {
    free((uv_async_t *)handle->data);
}

/* ---------- helpers ---------- */

static WCHAR *to_utf16(const char *str) {
    int    len = MultiByteToWideChar(CP_UTF8, 0, str, -1, NULL, 0);
    if (len <= 0) return NULL;
    WCHAR *wstr = xmalloc((size_t)(len + 1) * sizeof(WCHAR));
    if (MultiByteToWideChar(CP_UTF8, 0, str, -1, wstr, len) != len) {
        free(wstr);
        return NULL;
    }
    wstr[len] = L'\0';
    return wstr;
}

/* Quote a single argv[i] per CommandLineToArgvW rules. Caller frees if != arg. */
static char *quote_arg(const char *arg) {
    bool needs_quote = (arg[0] == '\0') || strpbrk(arg, " \t\"") != NULL;
    if (!needs_quote) return (char *)arg;

    size_t len = strlen(arg);
    /* Worst case: every char gets escaped + surrounding quotes */
    char  *out = xmalloc(len * 2 + 3);
    char  *p   = out;
    *p++ = '"';
    for (size_t i = 0; i < len; i++) {
        int backslashes = 0;
        while (i < len && arg[i] == '\\') { backslashes++; i++; }
        if (i == len) {
            for (int j = 0; j < backslashes * 2; j++) *p++ = '\\';
            break;
        } else if (arg[i] == '"') {
            for (int j = 0; j < backslashes * 2 + 1; j++) *p++ = '\\';
            *p++ = '"';
        } else {
            for (int j = 0; j < backslashes; j++) *p++ = '\\';
            *p++ = arg[i];
        }
    }
    *p++ = '"';
    *p   = '\0';
    return out;
}

static WCHAR *join_args(char **argv) {
    size_t total          = 1; /* trailing NUL */
    char **freeable       = xmalloc(sizeof(char *) * 64);
    int    freeable_count = 0;

    for (char **p = argv; *p; p++) {
        char *q = quote_arg(*p);
        if (q != *p && freeable_count < 64) freeable[freeable_count++] = q;
        total += strlen(q) + 1;
    }

    char *buf = xmalloc(total);
    buf[0] = '\0';
    bool  first = true;
    for (char **p = argv; *p; p++) {
        char *q = quote_arg(*p);
        if (!first) strcat(buf, " ");
        strcat(buf, q);
        first = false;
    }

    WCHAR *wide = to_utf16(buf);
    free(buf);
    for (int i = 0; i < freeable_count; i++) free(freeable[i]);
    free(freeable);
    return wide;
}

/* ---------- API ---------- */

void pty_impl_destroy(pty_process *process) {
    pty_impl_t *impl = process->impl;
    if (impl == NULL) return;
    if (impl->wait != NULL) UnregisterWaitEx(impl->wait, INVALID_HANDLE_VALUE);
    if (impl->hpc != NULL) ClosePseudoConsole(impl->hpc);
    if (impl->hProcess != NULL) CloseHandle(impl->hProcess);
    if (impl->hThread != NULL) CloseHandle(impl->hThread);
    if (impl->attr_list != NULL) {
        DeleteProcThreadAttributeList(impl->attr_list);
        free(impl->attr_list);
    }
    free(impl->in_pipe_name);
    free(impl->out_pipe_name);
    free(impl);
    process->impl = NULL;
}

bool pty_resize(pty_process *process) {
    if (process == NULL || process->impl == NULL) return false;
    if (process->columns <= 0 || process->rows <= 0) return false;
    COORD size = { (SHORT)process->columns, (SHORT)process->rows };
    return SUCCEEDED(ResizePseudoConsole(process->impl->hpc, size));
}

bool pty_kill(pty_process *process, int sig) {
    (void)sig; /* Windows has no signals; SIGTERM/SIGKILL both → TerminateProcess */
    if (process == NULL || process->impl == NULL) return false;
    return TerminateProcess(process->impl->hProcess, 1) != 0;
}

pid_t pty_get_fg_pid(pty_process *process) {
    /* Windows has no foreground process group; report the spawned process pid. */
    if (process == NULL) return -1;
    return (pid_t)process->pid;
}

/* ---------- spawn ---------- */

static void connect_cb(uv_connect_t *req, int status) {
    if (status != 0) {
        fprintf(stderr, "pty_win32: uv_pipe_connect failed: %s\n", uv_strerror(status));
    }
    free(req);
}

static VOID CALLBACK on_process_exit(PVOID context, BOOLEAN timed_out) {
    (void)timed_out;
    pty_process *process = (pty_process *)context;
    uv_async_send(&process->async);
}

static void async_cb(uv_async_t *async) {
    pty_process *process = (pty_process *)async->data;
    DWORD        code    = 0;
    if (process->impl && process->impl->hProcess) {
        GetExitCodeProcess(process->impl->hProcess, &code);
    }
    process->exit_code   = (int)code;
    process->exit_signal = 0;
    process->exit_cb(process);
    uv_close((uv_handle_t *)async, async_free_cb);
    process_free(process);
}

static bool create_pipes(pty_impl_t *impl, HANDLE *pty_in, HANDLE *pty_out) {
    static LONG counter = 0;
    LONG        n       = InterlockedIncrement(&counter);
    DWORD       pid     = GetCurrentProcessId();

    char        in_name[128], out_name[128];
    snprintf(in_name, sizeof(in_name),  "\\\\.\\pipe\\tabsh-in-%lu-%ld",  pid, n);
    snprintf(out_name, sizeof(out_name), "\\\\.\\pipe\\tabsh-out-%lu-%ld", pid, n);
    impl->in_pipe_name  = strdup(in_name);
    impl->out_pipe_name = strdup(out_name);

    SECURITY_ATTRIBUTES sa        = { sizeof(sa), NULL, FALSE };
    const DWORD         open_mode = PIPE_ACCESS_INBOUND | PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE;
    const DWORD         pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT;

    *pty_in  = CreateNamedPipeA(impl->in_pipe_name,  open_mode, pipe_mode, 1, 0, 0, 30000, &sa);
    *pty_out = CreateNamedPipeA(impl->out_pipe_name, open_mode, pipe_mode, 1, 0, 0, 30000, &sa);
    return *pty_in != INVALID_HANDLE_VALUE && *pty_out != INVALID_HANDLE_VALUE;
}

int pty_spawn(pty_process *process, pty_read_cb read_cb, pty_exit_cb exit_cb) {
    pty_impl_t *impl = xmalloc(sizeof(pty_impl_t));
    memset(impl, 0, sizeof(*impl));

    HANDLE pty_in = INVALID_HANDLE_VALUE, pty_out = INVALID_HANDLE_VALUE;
    WCHAR *cmdline = NULL, *cwd_w = NULL;
    int    status = 1;

    if (!create_pipes(impl, &pty_in, &pty_out)) {
        fprintf(stderr, "pty_win32: CreateNamedPipeA failed\n");
        goto fail;
    }

    COORD size = { (SHORT)process->columns, (SHORT)process->rows };
    if (FAILED(CreatePseudoConsole(size, pty_in, pty_out, 0, &impl->hpc))) {
        fprintf(stderr, "pty_win32: CreatePseudoConsole failed\n");
        goto fail;
    }
    /* ConPTY owns these now */
    CloseHandle(pty_in);  pty_in  = INVALID_HANDLE_VALUE;
    CloseHandle(pty_out); pty_out = INVALID_HANDLE_VALUE;

    /* Build STARTUPINFOEX with PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE */
    STARTUPINFOEXW si_ex = { 0 };
    si_ex.StartupInfo.cb = sizeof(si_ex);

    SIZE_T attr_size = 0;
    InitializeProcThreadAttributeList(NULL, 1, 0, &attr_size);
    impl->attr_list = xmalloc(attr_size);
    if (!InitializeProcThreadAttributeList(impl->attr_list, 1, 0, &attr_size)) {
        fprintf(stderr, "pty_win32: InitializeProcThreadAttributeList failed\n");
        goto fail;
    }
    si_ex.lpAttributeList = impl->attr_list;
    if (!UpdateProcThreadAttribute(si_ex.lpAttributeList, 0,
                                   PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                                   impl->hpc, sizeof(HPCON), NULL, NULL)) {
        fprintf(stderr, "pty_win32: UpdateProcThreadAttribute failed\n");
        goto fail;
    }

    cmdline = join_args(process->argv);
    if (cmdline == NULL) goto fail;
    if (process->cwd != NULL) {
        cwd_w = to_utf16(process->cwd);
        if (cwd_w == NULL) goto fail;
    }
    if (process->envp != NULL) {
        /* For now, mutate parent env. A clean impl would build a per-process env block. */
        for (char **p = process->envp; *p; p++) {
            WCHAR *w = to_utf16(*p);
            if (w == NULL) continue;
            _wputenv(w);
            free(w);
        }
    }

    PROCESS_INFORMATION pi    = { 0 };
    DWORD               flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;
    if (!CreateProcessW(NULL, cmdline, NULL, NULL, FALSE, flags, NULL, cwd_w,
                        &si_ex.StartupInfo, &pi)) {
        fprintf(stderr, "pty_win32: CreateProcessW failed (err=%lu)\n", GetLastError());
        goto fail;
    }
    impl->hProcess = pi.hProcess;
    impl->hThread  = pi.hThread;

    /* Wire pipes into libuv via the named-pipe server endpoint we created.
     * uv_pipe_connect connects as a client to the same name; libuv opens it
     * with FILE_FLAG_OVERLAPPED, which is what it needs. */
    process->in  = xmalloc(sizeof(uv_pipe_t));
    process->out = xmalloc(sizeof(uv_pipe_t));
    uv_pipe_init(process->loop, process->in, 0);
    uv_pipe_init(process->loop, process->out, 0);
    uv_connect_t *in_req  = xmalloc(sizeof(uv_connect_t));
    uv_connect_t *out_req = xmalloc(sizeof(uv_connect_t));
    uv_pipe_connect(in_req,  process->in,  impl->in_pipe_name,  connect_cb);
    uv_pipe_connect(out_req, process->out, impl->out_pipe_name, connect_cb);

    process->impl       = impl;
    process->pid        = (int)pi.dwProcessId;
    process->paused     = true;
    process->read_cb    = read_cb;
    process->exit_cb    = exit_cb;
    process->async.data = process;
    uv_async_init(process->loop, &process->async, async_cb);

    if (!RegisterWaitForSingleObject(&impl->wait, pi.hProcess, on_process_exit,
                                     process, INFINITE, WT_EXECUTEONLYONCE)) {
        fprintf(stderr, "pty_win32: RegisterWaitForSingleObject failed\n");
        goto fail;
    }

    status = 0;
    goto done;

fail:
    if (pty_in  != INVALID_HANDLE_VALUE) CloseHandle(pty_in);
    if (pty_out != INVALID_HANDLE_VALUE) CloseHandle(pty_out);
    if (impl) {
        if (impl->hpc) ClosePseudoConsole(impl->hpc);
        if (impl->attr_list) {
            DeleteProcThreadAttributeList(impl->attr_list);
            free(impl->attr_list);
        }
        free(impl->in_pipe_name);
        free(impl->out_pipe_name);
        free(impl);
    }

done:
    free(cmdline);
    free(cwd_w);
    return status;
}
