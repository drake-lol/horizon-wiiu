/* Measure devkitPPC newlib's pthread types: powerpc-eabi-gcc -c probe_pthread.c && powerpc-eabi-nm -S probe_pthread.o */
#include <pthread.h>
char m[sizeof(pthread_mutex_t)]; char c[sizeof(pthread_cond_t)]; char a[sizeof(pthread_mutexattr_t)];
char r[sizeof(pthread_rwlock_t)]; char at[sizeof(pthread_attr_t)]; char ca[sizeof(pthread_condattr_t)];
pthread_mutex_t mi = PTHREAD_MUTEX_INITIALIZER; pthread_cond_t ci = PTHREAD_COND_INITIALIZER;
