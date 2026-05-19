#ifndef TTYD_UTIL_H
#define TTYD_UTIL_H

#include <stddef.h>
#include <stdbool.h>

// malloc with NULL check
void *xmalloc(size_t size);

// realloc with NULL check
void *xrealloc(void *p, size_t size);

// Get human readable signal string
int get_sig_name(int sig, char *buf, size_t len);

// Get signal code from string like SIGHUP
int get_sig(const char *sig_name);


#ifdef _WIN32
char *strsep(char **sp, char *sep);
#endif
#endif  // TTYD_UTIL_H
