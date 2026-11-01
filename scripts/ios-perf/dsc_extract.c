// The iOS 27 simulator's system frameworks exist only inside its dyld shared cache
// (RuntimeRoot/System/Library/Caches/com.apple.dyld/dyld_sim_shared_cache_arm64), and a Time
// Profiler trace attached to a simulator app leaves their frames as bare addresses. `atos`
// needs the images on disk, so extract them with Apple's own extractor bundle:
//   xcrun clang -fblocks -o dscx dsc_extract.c && taskpolicy -b ./dscx <cache> <out dir>
#include <dlfcn.h>
#include <stdio.h>
typedef int (*extract_fn)(const char *cache, const char *root, void (^progress)(unsigned, unsigned));
int main(int argc, char **argv) {
    void *h = dlopen("/usr/lib/dsc_extractor.bundle", RTLD_LAZY);
    if (!h) { fprintf(stderr, "dlopen: %s\n", dlerror()); return 2; }
    extract_fn f = (extract_fn)dlsym(h, "dyld_shared_cache_extract_dylibs_progress");
    if (!f) { fprintf(stderr, "no symbol\n"); return 3; }
    return f(argv[1], argv[2], ^(unsigned c, unsigned t) { if (c % 200 == 0) fprintf(stderr, "%u/%u\n", c, t); });
}
