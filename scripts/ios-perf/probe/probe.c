// Side-effect counter for the first-open warm-up (perf rig, measurement only; never shipped).
// Loaded with DYLD_INSERT_LIBRARIES. Every hooked call is counted against the warm-up window
// that `arbos_probe_phase` brackets (1-2: ChatSession construction on the main thread, 3-4: the
// detached warming task). "own" = on the warm-up's own thread inside its own sub-window, which
// is exclusively warm-up code: both sub-windows are synchronous, with no suspension point.
#include <CoreFoundation/CoreFoundation.h>
#include <arpa/inet.h>
#include <dispatch/dispatch.h>
#include <execinfo.h>
#include <netinet/in.h>
#include <objc/message.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <objc/runtime.h>
#include <os/log.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "names.h"

extern void *hk_orig[HK_N];
extern void *hk_stubs[HK_N];

static const char *hk_name[HK_N];
static const char *hk_cat[HK_N];
static int hk_check[HK_N];
static _Atomic long c_before[HK_N], c_own[HK_N], c_other[HK_N], c_after[HK_N];
static _Atomic int phase;
static _Atomic uint64_t tid1, tid3;
static __thread int busy;
static os_log_t L;

// connect() can run inside another image's initializer, before this library's constructor.
static os_log_t log_handle(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{ L = os_log_create("com.unarbos.arbos.perfprobe", "probe"); });
    return L;
}

static uint64_t tid(void) {
    uint64_t t;
    pthread_threadid_np(NULL, &t);
    return t;
}

static void dump(const char *when) {
    long own = 0, other = 0;
    for (int i = 0; i < HK_N; i++) {
        if (!hk_name[i]) continue;
        long b = c_before[i], o = c_own[i], x = c_other[i], a = c_after[i];
        own += o;
        other += x;
        if (b || o || x || a)
            os_log(L, "probe count when=%{public}s cat=%{public}s name=%{public}s before=%ld own=%ld other=%ld after=%ld",
                   when, hk_cat[i], hk_name[i], b, o, x, a);
    }
    int armed = 0;
    for (int i = 0; i < HK_N; i++) armed += hk_name[i] != NULL;
    os_log(L, "probe summary when=%{public}s phase=%d armed=%d own_total=%ld other_in_window_total=%ld",
           when, (int)phase, armed, own, other);
}

static const char *image_of(uint64_t pc, uint64_t *off) {
    Dl_info info = {0};
    if (!dladdr((void *)pc, &info) || !info.dli_fname) return "?";
    *off = pc - (uint64_t)info.dli_fbase;
    const char *slash = strrchr(info.dli_fname, '/');
    return slash ? slash + 1 : info.dli_fname;
}

static id msg(id obj, const char *sel) {
    return obj ? ((id(*)(id, SEL))objc_msgSend)(obj, sel_registerName(sel)) : NULL;
}

static const char *utf8(id str) {
    return str ? ((const char *(*)(id, SEL))objc_msgSend)(str, sel_registerName("UTF8String")) : NULL;
}

// Network hits say where they go: the socket address of a connect(), the URL, method and
// header names of a URLSession request, and the frames that started it.
static void describe_network(long i, uint64_t *regs, uint64_t lr) {
    char what[640] = "";
    const char *name = hk_name[i] ? hk_name[i] : i < HK_NC ? hk_name_c[i] : "?";
    if (!strcmp(name, "connect")) {
        const struct sockaddr *sa = (const struct sockaddr *)regs[1];
        char addr[128] = "?";
        int port = -1;
        if (sa && sa->sa_family == AF_INET) {
            inet_ntop(AF_INET, &((const struct sockaddr_in *)sa)->sin_addr, addr, sizeof addr);
            port = ntohs(((const struct sockaddr_in *)sa)->sin_port);
        } else if (sa && sa->sa_family == AF_INET6) {
            inet_ntop(AF_INET6, &((const struct sockaddr_in6 *)sa)->sin6_addr, addr, sizeof addr);
            port = ntohs(((const struct sockaddr_in6 *)sa)->sin6_port);
        } else if (sa && sa->sa_family == AF_UNIX) {
            snprintf(addr, sizeof addr, "unix:%s", ((const struct sockaddr_un *)sa)->sun_path);
        }
        snprintf(what, sizeof what, "fd=%d family=%d addr=%s port=%d", (int)regs[0], sa ? sa->sa_family : -1, addr, port);
    } else if (name[0] == '-') {
        id req = NULL;
        if (strstr(name, " resume]")) req = msg((id)regs[0], "currentRequest");
        else if (strstr(name, "WithRequest:")) req = (id)regs[2];
        if (req) {
            id keys = msg(msg(req, "allHTTPHeaderFields"), "allKeys");
            id joined = keys ? ((id(*)(id, SEL, id))objc_msgSend)(keys, sel_registerName("componentsJoinedByString:"), (id)CFSTR(",")) : NULL;
            const char *u = utf8(msg(msg(req, "URL"), "absoluteString")), *m = utf8(msg(req, "HTTPMethod")), *h = utf8(joined);
            snprintf(what, sizeof what, "method=%s url=%s headers=[%s]", m ? m : "?", u ? u : "?", h ? h : "");
        }
    }
    void *bt[24];
    int n = backtrace(bt, 24);
    char callers[1200] = "";
    size_t len = 0;
    for (int k = 1; k < n && len < sizeof callers - 64; k++) {
        uint64_t off = 0;
        const char *img = image_of((uint64_t)bt[k], &off);
        if (!strcmp(img, "probe.dylib")) continue;
        len += snprintf(callers + len, sizeof callers - len, " %s+0x%llx", img, (unsigned long long)off);
    }
    uint64_t off = 0;
    const char *img = image_of(lr, &off);
    os_log(log_handle(), "probe NET phase=%d main=%d name=%{public}s %{public}s caller=%{public}s+0x%llx stack=%{public}s",
           (int)phase, pthread_main_np(), name, what, img, (unsigned long long)off, callers);
}

void arbos_hit(long i, uint64_t *regs, uint64_t lr) {
    if (busy || i < 0 || i >= HK_N) return;
    busy = 1;
    int check = hk_check[i];
    int writeish = 1;
    if (check == 1 || check == 2) {
        long flags = (long)regs[check];
        writeish = (flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND)) != 0;
    } else if (check == 3) {
        const char *mode = (const char *)regs[1];
        writeish = mode && (strchr(mode, 'w') || strchr(mode, 'a') || strchr(mode, '+'));
    }
    const char *cat = hk_cat[i] ? hk_cat[i] : i < HK_NC ? hk_cat_c[i] : NULL;
    if (writeish && cat && !strcmp(cat, "network")) describe_network(i, regs, lr);
    if (writeish) {
        int p = phase;
        uint64_t t = tid();
        if (p == 0) c_before[i]++;
        else if (p >= 4) c_after[i]++;
        else if ((p == 1 && t == tid1) || (p == 3 && t == tid3)) {
            c_own[i]++;
            Dl_info info = {0};
            dladdr((void *)lr, &info);
            os_log(L, "probe OWN hit phase=%d cat=%{public}s name=%{public}s caller=%{public}s+0x%llx",
                   p, hk_cat[i], hk_name[i], info.dli_fname ? strrchr(info.dli_fname, '/') + 1 : "?",
                   (unsigned long long)(lr - (uint64_t)info.dli_fbase));
        } else {
            c_other[i]++;
        }
    }
    busy = 0;
}

void arbos_probe_phase(int p) {
    uint64_t t = tid();
    if (p == 1) tid1 = t;
    if (p == 3) tid3 = t;
    phase = p;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    os_log(L, "probe phase=%d tid=%llu main=%d wall=%ld.%06ld", p, (unsigned long long)t, pthread_main_np(),
           (long)ts.tv_sec, (long)(ts.tv_nsec / 1000));
    if (p == 4)
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_global_queue(0, 0),
                       ^{ dump("phase4+3s"); });
}

static int next_slot = HK_NC;

static void hook_method(Class cls, Method m, const char *prefix, const char *cat) {
    if (next_slot >= HK_N) return;
    int k = next_slot++;
    char *name = malloc(256);
    snprintf(name, 256, "%s[%s %s]", prefix, class_getName(cls), sel_getName(method_getName(m)));
    hk_name[k] = name;
    hk_cat[k] = cat;
    hk_orig[k] = (void *)method_getImplementation(m);
    method_setImplementation(m, (IMP)hk_stubs[k]);
}

static int inherits(Class c, Class base) {
    for (; c; c = class_getSuperclass(c))
        if (c == base) return 1;
    return 0;
}

static int has_prefix(const char *s, const char *p) { return strncmp(s, p, strlen(p)) == 0; }

static void hook_objc(void) {
    Class task = objc_getClass("NSURLSessionTask"), session = objc_getClass("NSURLSession");
    Class center = objc_getClass("NSNotificationCenter"), defaults = objc_getClass("NSUserDefaults");
    unsigned int nc = 0;
    Class *classes = objc_copyClassList(&nc);
    for (unsigned int ci = 0; ci < nc; ci++) {
        Class c = classes[ci];
        int is_task = task && inherits(c, task), is_session = session && inherits(c, session);
        int is_center = center && c == center, is_defaults = defaults && c == defaults;
        if (!(is_task || is_session || is_center || is_defaults)) continue;
        unsigned int nm = 0;
        Method *ms = class_copyMethodList(c, &nm);
        for (unsigned int mi = 0; mi < nm; mi++) {
            const char *sel = sel_getName(method_getName(ms[mi]));
            if (is_task && (!strcmp(sel, "resume") || has_prefix(sel, "sendMessage:")))
                hook_method(c, ms[mi], "-", "network");
            else if (is_session && (strstr(sel, "TaskWith") || strstr(sel, "taskWith")))
                hook_method(c, ms[mi], "-", "network");
            else if (is_center && has_prefix(sel, "addObserver"))
                hook_method(c, ms[mi], "-", "listener");
            else if (is_defaults && (has_prefix(sel, "set") || has_prefix(sel, "removeObjectForKey")) &&
                     strchr(sel, ':') && strstr(sel, "orKey:"))
                hook_method(c, ms[mi], "-", "defaults");
        }
        free(ms);
    }
    free(classes);
}

__attribute__((constructor)) static void init(void) {
    log_handle();
    for (int i = 0; i < HK_NC; i++) {
        if (!hk_orig[i]) continue;
        hk_name[i] = hk_name_c[i];
        hk_cat[i] = hk_cat_c[i];
        hk_check[i] = hk_check_c[i];
    }
    hook_objc();
    os_log(L, "probe loaded interposed=%d objc=%d", HK_NC, next_slot - HK_NC);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 14 * NSEC_PER_SEC), dispatch_get_global_queue(0, 0),
                   ^{ dump("t+14s"); });
}
