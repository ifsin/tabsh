package main

import (
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	cpty "github.com/creack/pty"
)

func spawnPTY(app *AppConfig, cols, rows uint16, cwd, termType string) (*os.File, error) {
	argv := []string{app.Command}
	argv = append(argv, app.Args...)

	shimDir := getShimDir()
	shellBase := strings.ToLower(filepath.Base(app.Command))

	login := false
	if shimDir != "" {
		switch shellBase {
		case "bash":
			kept := make([]string, 0, len(argv)+2)
			kept = append(kept, argv[0])
			for _, a := range argv[1:] {
				if a == "-l" || a == "--login" {
					login = true
					continue
				}
				kept = append(kept, a)
			}
			argv = append(kept, "--rcfile", filepath.Join(shimDir, "init", "bashrc"))
		case "fish":
			argv = append(argv, "--init-command", "source '"+filepath.Join(shimDir, "init", "tabsh.fish")+"'")
		case "pwsh", "powershell":
			argv = append(argv, "-NoExit", "-Command", ". '"+filepath.Join(shimDir, "init", "init.ps1")+"'")
		}
	}

	cmd := exec.Command(argv[0], argv[1:]...)

	if cwd != "" {
		cmd.Dir = cwd
	} else if app.Cwd != "" {
		cmd.Dir = app.Cwd
	}

	cmd.Env = buildEnv(shellBase, shimDir, login, termType, app.Env)

	ptmx, err := cpty.StartWithSize(cmd, &cpty.Winsize{Cols: cols, Rows: rows})
	if err != nil {
		return nil, err
	}
	go func() {
		if err := cmd.Wait(); err != nil {
			log.Printf("process exited: %v", err)
		}
	}()
	return ptmx, nil
}

func buildEnv(shellBase, shimDir string, login bool, termType string, extra map[string]string) []string {
	env := os.Environ()
	set := func(k, v string) {
		env = append(env, k+"="+v)
	}
	if termType == "" {
		termType = "xterm-256color"
	}
	set("TERM", termType)
	set("COLORTERM", "truecolor")
	for k, v := range extra {
		set(k, v)
	}

	if shimDir == "" {
		return env
	}
	set("TABSH_SHIM_DIR", shimDir)

	switch shellBase {
	case "zsh":
		orig := os.Getenv("ZDOTDIR")
		if orig == "" {
			orig, _ = os.UserHomeDir()
		}
		set("TABSH_ORIG_ZDOTDIR", orig)
		set("ZDOTDIR", filepath.Join(shimDir, "init", "zsh"))

	case "bash":
		if login {
			set("TABSH_LOGIN", "1")
		}

	case "sh", "dash", "ash", "ksh":
		if orig := os.Getenv("ENV"); orig != "" {
			set("TABSH_ORIG_ENV", orig)
		}
		set("ENV", filepath.Join(shimDir, "init", "env.sh"))
	}

	return env
}
