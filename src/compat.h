/* compat.h -- minimal POSIX helpers */
#ifndef TTYD_COMPAT_H
#define TTYD_COMPAT_H

#include <sys/stat.h>
#define mkdir_p(path, mode) mkdir(path, mode)

#endif /* TTYD_COMPAT_H */
