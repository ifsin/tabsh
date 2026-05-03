/* compat.h -- MSVC compatibility shims for POSIX functions */
#ifndef TTYD_COMPAT_H
#define TTYD_COMPAT_H

#ifdef _MSC_VER
#include <string.h>
#include <sys/stat.h>
#define strcasecmp _stricmp
#define strncasecmp _strnicmp

typedef int pid_t;

#ifndef S_ISDIR
#define S_ISDIR(m) (((m) & _S_IFMT) == _S_IFDIR)
#endif
#ifndef S_ISREG
#define S_ISREG(m) (((m) & _S_IFMT) == _S_IFREG)
#endif

static inline char *strcasestr(const char *haystack, const char *needle) {
  if (!*needle) return (char *)haystack;
  for (; *haystack; haystack++) {
    if (_strnicmp(haystack, needle, strlen(needle)) == 0)
      return (char *)haystack;
  }
  return NULL;
}
#endif

#ifdef _WIN32
#include <direct.h>
#define mkdir_p(path, mode) _mkdir(path)
#else
#define mkdir_p(path, mode) mkdir(path, mode)
#endif

#endif /* TTYD_COMPAT_H */
