#!/usr/bin/env python3
# Generates the side-effect counter (perf rig, measurement only): hooks.s with one trampoline per
# hooked function, the dyld __interpose tuples, and the table of originals; names.h with labels.
# The app brackets the window by calling `arbos_probe_phase(1...4)`, looked up with dlsym, from a
# measurement-only build; `perf-rig.sh probebuild` builds it and `proberun` injects it.
# usage: gen.py <imports.txt> <outdir>
import sys

imports = [l.strip() for l in open(sys.argv[1]) if l.strip()]
out = sys.argv[2]

# (symbol, category, argcheck) argcheck: 0 none, 1 open flags x1, 2 openat flags x2, 3 fopen mode x1
fixed = [
    ("_SecItemAdd", "keychain", 0), ("_SecItemCopyMatching", "keychain", 0),
    ("_SecItemUpdate", "keychain", 0), ("_SecItemDelete", "keychain", 0),
    ("_CFNotificationCenterAddObserver", "listener", 0),
    ("_notify_register_dispatch", "listener", 0), ("_notify_register_check", "listener", 0),
    ("_CFPreferencesSetAppValue", "defaults", 0),
    ("_$s7Combine25ObservableObjectPublisherC4sendyyF", "publish", 0),
    ("_swift_task_create", "task", 0),
    ("_connect", "network", 0), ("_getaddrinfo", "network", 0), ("_nw_connection_start", "network", 0),
    ("_open", "disk-write", 1), ("_openat", "disk-write", 2), ("_fopen", "disk-write", 3),
    ("_write", "disk-write", 0), ("_pwrite", "disk-write", 0), ("_rename", "disk-write", 0),
    ("_unlink", "disk-write", 0), ("_mkdir", "disk-write", 0), ("_fsync", "disk-write", 0),
]
syms = fixed + [(s, "ffi", 0) for s in imports if s.startswith("_uniffi_")]
OBJC_SLOTS = 160
n_c = len(syms)
n = n_c + OBJC_SLOTS

s = [".text", ".p2align 2"]
for i in range(n):
    s += [f"_stub_{i}:", f"  mov x16, #{i}", "  b _hk_common"]
s += ["""
.p2align 2
_hk_common:
  sub sp, sp, #240
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp x8, x20, [sp, #64]
  stp x21, x16, [sp, #80]
  stp x29, x30, [sp, #96]
  add x29, sp, #96
  stp q0, q1, [sp, #112]
  stp q2, q3, [sp, #144]
  stp q4, q5, [sp, #176]
  stp q6, q7, [sp, #208]
  mov x0, x16
  mov x1, sp
  mov x2, x30
  bl _arbos_hit
  ldp q6, q7, [sp, #208]
  ldp q4, q5, [sp, #176]
  ldp q2, q3, [sp, #144]
  ldp q0, q1, [sp, #112]
  ldp x21, x16, [sp, #80]
  ldp x8, x20, [sp, #64]
  ldp x6, x7, [sp, #48]
  ldp x4, x5, [sp, #32]
  ldp x2, x3, [sp, #16]
  ldp x0, x1, [sp, #0]
  adrp x17, _hk_orig@PAGE
  add x17, x17, _hk_orig@PAGEOFF
  ldr x16, [x17, x16, lsl #3]
  ldp x29, x30, [sp, #96]
  add sp, sp, #240
  br x16
"""]
s += [".data", ".p2align 3", ".globl _hk_orig", "_hk_orig:"]
for sym, _, _ in syms:
    s.append(f'  .quad "{sym}"')
s.append(f"  .space {OBJC_SLOTS * 8}")
s += [".globl _hk_stubs", ".p2align 3", "_hk_stubs:"]
for i in range(n):
    s.append(f"  .quad _stub_{i}")
s += ['.section __DATA,__interpose', ".p2align 3"]
for i, (sym, _, _) in enumerate(syms):
    s += [f"  .quad _stub_{i}", f'  .quad "{sym}"']
for sym, _, _ in syms:
    s.append(f'.weak_reference "{sym}"')
open(f"{out}/hooks.s", "w").write("\n".join(s) + "\n")

h = [f"#define HK_NC {n_c}", f"#define HK_N {n}"]
h.append("static const char *hk_name_c[HK_NC] = {" + ",".join(f'"{x[0][1:]}"' for x in syms) + "};")
h.append("static const char *hk_cat_c[HK_NC] = {" + ",".join(f'"{x[1]}"' for x in syms) + "};")
h.append("static const int hk_check_c[HK_NC] = {" + ",".join(str(x[2]) for x in syms) + "};")
open(f"{out}/names.h", "w").write("\n".join(h) + "\n")
print(f"{n_c} interposed, {OBJC_SLOTS} objc slots")
