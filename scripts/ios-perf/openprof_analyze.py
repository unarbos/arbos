#!/usr/bin/env python3
"""Break down main-thread time inside `open-chat` signpost intervals of a Time Profiler trace.

Inputs (from `xctrace export` on the gate Mac):
  <name>.tp.xml   table schema="time-profile"
  <name>.sp.xml   table schema="os-signpost"
Usage: openprof_analyze.py tp.xml sp.xml label GROUPS
  GROUPS: `first=0;repeat=1-` or `dm-first=0;group-first=1;dm-repeat=2-:2;group-repeat=3-:2`
  (open indices: `a-b`, `a-` to the end, `:k` step).
Per group: interval and main-thread CPU per open, self time by binary, time charged to the
deepest app frame (additive), named categories (inclusive, overlapping), a pruned call tree,
and, for a pair `X-first` / `X-repeat`, the first-minus-repeat difference by frame.
"""
import sys, re, collections, statistics
import xml.etree.ElementTree as ET

APP_BINARIES = ("Arbos",)


def load(path):
    ids = {}

    def resolve(e):
        r = e.get("ref")
        return ids.get(r, e) if r else e

    def index(e):
        if e.get("id"):
            ids[e.get("id")] = e
        for c in e:
            index(c)

    for ev, el in ET.iterparse(path, events=("end",)):
        if el.tag == "row":
            index(el)
            yield {c.tag: resolve(c) for c in el}, resolve


SYMS = {}


def load_syms(path):
    """addr<TAB>atos output, for system frames the trace left as bare addresses."""
    for line in open(path):
        a, s = line.rstrip("\n").split("\t", 1)
        SYMS[a] = re.sub(r" \(in [^)]+\) \+ \d+$", "", s)


def frames(bt, resolve):
    out = []
    if bt is None:
        return out
    for f in bt.iter("frame"):
        f = resolve(f)
        b = f.find("binary")
        b = resolve(b) if b is not None else None
        name = f.get("name") or f.get("addr") or "?"
        if name.startswith("0x"):
            name = SYMS.get(f.get("addr") or name, name)
        out.append((name, b.get("name") if b is not None else "?"))
    return out


def intervals_of(sp):
    """All signpost intervals: name -> [(begin, end, end message)], paired by (name, identifier)."""
    begins, out = {}, collections.defaultdict(list)
    for row, res in load(sp):
        name = row.get("signpost-name")
        if name is None:
            continue
        name = (name.text or "").strip()
        t = int(row["event-time"].text)
        et = row.get("event-type")
        et = et.text if et is not None else ""
        ident = row.get("os-signpost-identifier")
        key = (name, ident.text if ident is not None else "")
        msg = row.get("os-log-metadata")
        msg = msg.get("fmt") or "" if msg is not None else ""
        if et.startswith("Begin"):
            begins[key] = t
        elif et.startswith("End") and key in begins:
            out[name].append((begins.pop(key), t, msg))
    # The export can carry the same signpost table more than once.
    return {k: sorted(set(v)) for k, v in out.items()}


def samples_of(tp, spans):
    out = []
    starts = [a for a, _, _ in spans]
    for row, res in load(tp):
        th = row.get("thread")
        if th is None:
            continue
        tname = th.get("fmt") or ""
        if "Main Thread" not in tname:
            continue
        t = int(row["sample-time"].text)
        k = next((i for i, (a, b, _) in enumerate(spans) if a <= t <= b), None)
        if k is None:
            continue
        w = row.get("weight")
        wms = int(w.text) / 1e6 if w is not None and w.text and w.text.isdigit() else 1.0
        bt = row.get("backtrace")
        fr = frames(bt if bt is not None else row.get("tagged-backtrace"), res)
        if fr:
            out.append((k, t - spans[k][0], wms, fr))
    return out


def parse_groups(spec, n):
    groups = []
    for part in spec.split(";"):
        name, rng = part.split("=")
        step = 1
        if ":" in rng:
            rng, step = rng.split(":")
            step = int(step)
        if "-" in rng:
            a, b = rng.split("-")
            a, b = int(a), (int(b) if b else n - 1)
        else:
            a = b = int(rng)
        groups.append((name, [i for i in range(a, b + 1, step) if i < n]))
    return groups


CATEGORIES = [
    ("markdown parse (Rust core over FFI: markdownBlocks/markdownSpans, uniffi)",
     re.compile(r"markdownBlocks|markdownSpans|uniffi|arbos_core|makeRustCall")),
    ("Markdown.* Swift side (blocks/nest/inlineText/plain/polish)", re.compile(r"\bMarkdown\.")),
    ("Transcript row build (refreshRows / Transcript.rows / proseBlocks)", re.compile(r"refreshRows|Transcript\.|proseBlocks")),
    ("Text layout and shaping (CoreText / TextKit / UIFoundation)", re.compile(r"^CT|TypeSetter|Typesetter|TextLayout|NSLayoutManager|NSTextStorage|_NSAttributed|TextRendering|CoreText")),
    ("Font lookup (CTFont / FontDescriptor / FontParser)", re.compile(r"CTFont|TFont|FontDescriptor|FontParser|GSFont|UIFont")),
    ("Image decode / avatar (ImageIO, CGImage, AvatarImage)", re.compile(r"ImageIO|CGImageSource|copy_image|AvatarImage|AsyncImage|_cg_jpeg|png_read")),
    ("SwiftUI layout (sizeThatFits / placement / Lazy*)", re.compile(r"sizeThatFits|placeSubviews|LayoutComputer|LazyLayout|LazyVStack|LazySubview|StackLayout|ViewLayoutEngine")),
    ("AttributeGraph update (AG::Graph)", re.compile(r"^AG::|AGGraph|AGSubgraph")),
    ("Core Animation commit (CA::Transaction::commit)", re.compile(r"CA::Transaction::commit")),
    ("Core Animation layout (CA::Layer::layout_if_needed)", re.compile(r"layout_if_needed|layoutSublayers")),
    ("UIKit text input (UITextView / keyboard / TextField)", re.compile(r"UITextView|UIKeyboard|_UITextInput|UITextField|TextInputUI|UITextInputTraits|RTIInputSystem", re.I)),
    ("Navigation push (NavigationStack / UINavigationController)", re.compile(r"Navigation|pushViewController|_UIViewControllerTransition", re.I)),
    ("Liquid Glass / material (glassEffect / _UIGlass / backdrop)", re.compile(r"glass|Backdrop|Material", re.I)),
    ("ChatView body", re.compile(r"ChatView\.body|ChatView\.\w+\.getter|closure.*in ChatView")),
    ("HomeView body", re.compile(r"HomeView")),
    ("dyld / lazy symbol binding / class realization", re.compile(r"dyld|realizeClass|_objc_init|map_images|swift_getTypeByMangled|swift_conformsTo", re.I)),
    ("Swift metadata (swift_getGenericMetadata / conformances)", re.compile(r"swift_getGenericMetadata|swift_conformsToProtocol|swift_getWitnessTable|instantiateWitnessTable|getTypeByMangled")),
]


def _m(rx, nm, b):
    return rx.search(nm) or rx.search("bin:" + b)


# Work kind: what the CPU is computing. Named one-time causes win wherever they sit on the
# stack; otherwise the frame nearest the leaf that matches a generic kind decides. Additive.
ONE_TIME = [
    ("Metal device first use (RenderBox RBDevice.sharedDefaultDevice)", re.compile(r"RBDevice sharedDefaultDevice|MTLCreateSystemDefaultDevice|MTLDeviceArrayInitialize")),
    ("AttributedString → NSAttributedString scope reflection (AttributeScope.scopeDescription)", re.compile(r"AttributeScope\.scopeDescription")),
    ("SwiftUI view-type field reflection (DynamicPropertyCache.fields)", re.compile(r"DynamicPropertyCache\.fields")),
    ("Markdown parse in the Rust core (ArbosFFI)", re.compile(r"bin:ArbosFFI|markdownBlocks|markdownSpans|makeRustCall")),
]
KINDS = [
    ("Swift runtime type lookup (conformance scan, mangled-name metadata, demangler)",
     re.compile(r"swift_conformsToProtocol|instantiateConcreteTypeFromMangledName|getTypeByMangledName|swift::Demangle|swift_getGenericMetadata|_gatherGenericParameters|MetadataCacheKey|swift_getWitnessTable|mangled_type_name_ref|_dyld_find_protocol_conformance")),
    ("dyld / dlopen / ObjC class realization", re.compile(r"dlopen|dyld4::(?!start)|dyld3::|_dyld_|realizeClass|map_images|ImageLoader")),
    ("Transcript row build (refreshRows / Transcript.rows)", re.compile(r"ChatSession\.refreshRows|Transcript\.rows")),
    ("Composer UITextView create / measure (MentionTextView)", re.compile(r"MentionUITextView|MentionTextView|bin:UIKitCore.*UITextView")),
    ("Text layout and shaping (CoreText / TextKit 2 / UIFoundation)", re.compile(r"bin:CoreText|bin:UIFoundation|NSTextLayout|_performTextKit2Layout")),
    ("Image decode (ImageIO / CA copy_image)", re.compile(r"bin:ImageIO|copy_image|CGImageSource")),
    ("Render: Core Animation / RenderBox display and commit encode", re.compile(r"RBLayer display|CA::Layer::display|CA::Render::|CA::Context::commit|bin:RenderBox")),
    ("SwiftUI layout (sizeThatFits / LazyVStack placement)", re.compile(r"sizeThatFits|LazySubviewPlacements|LazyHVStack|lengthThatFits|placeChildren|LayoutComputer|childGeometries")),
    ("SwiftUI preferences (PreferenceCombiner / reduce)", re.compile(r"PreferenceCombiner|PreferenceKey\.reduce|preferenceValue")),
    ("SwiftUI view lists / ForEach diff (ViewList, DynamicContainer)", re.compile(r"ViewList|ForEachState|DynamicContainerInfo")),
    ("SwiftUI body evaluation (makeBody / DynamicBody / bodies)", re.compile(r"\.body\.getter|DynamicBody|makeBody|ViewBodyAccessor")),
    ("SwiftUI display list / render (DisplayList, ViewRendererHost.render)", re.compile(r"DisplayList|ViewRendererHost|updateDisplayList")),
    ("AttributeGraph bookkeeping (AG::Graph)", re.compile(r"bin:AttributeGraph|^AG::")),
    ("UIKit layout / view hierarchy", re.compile(r"layoutSubviews|layoutBelowIfNeeded|_layoutEngine|bin:UIKitCore")),
    ("malloc / retain-release / objc_msgSend", re.compile(r"bin:libsystem_malloc|swift_retain|swift_release|RefCounts|objc_msgSend|objc_retain|objc_release")),
]


def kind_of(fr):
    for name, rx in ONE_TIME:
        if any(_m(rx, nm, b) for nm, b in fr):
            return name
    for nm, b in fr:
        for name, rx in KINDS:
            if _m(rx, nm, b):
                return name
    return "other"


# Phase: which run-loop job the time belongs to — the match nearest the root.
PHASES = [
    ("UIKit push transition start (_UIViewControllerTransitioningRunCustomTransition…)", re.compile(r"_UIViewControllerTransitioningRunCustomTransition|startInteractiveTransition")),
    ("Core Animation layout + commit pass (CA::Transaction flush, layout_and_display_if_needed)", re.compile(r"CA::Transaction::flush|CA::Transaction::commit|layout_and_display_if_needed")),
    ("SwiftUI update after a state change (NSRunLoop.flushObservers / UpdateGroup)", re.compile(r"NSRunLoop\.flushObservers|UpdateGroup")),
    ("UIKit update sequence (_UIUpdateSequenceRunNext)", re.compile(r"_UIUpdateSequenceRun|__setupUpdateSequence")),
    ("Main-queue blocks / Swift tasks", re.compile(r"_dispatch_main_queue|swift_job_run|swift::runJob|__CFRUNLOOP_IS_SERVICING_THE_MAIN_DISPATCH_QUEUE__")),
    ("Timers / display link", re.compile(r"TIMER_CALLBACK|CADisplayLink|display_timer")),
    ("Events (touches, ports)", re.compile(r"__CFRunLoopDoSource1|__CFRunLoopDoSources0")),
]


def phase_of(fr):
    for name, rx in PHASES:
        if any(rx.search(nm) for nm, _ in fr):
            return name
    return "other"


def short(name, n=150):
    return name if len(name) <= n else name[: n - 1] + "…"


def deepest_app(fr):
    for name, binary in fr:
        if binary in APP_BINARIES:
            return name
    return None


def report(label, spans, samples, groups):
    print(f"#### {label}: {len(spans)} open-chat intervals")
    print("   per open ms:", " ".join(f"{(b - a) / 1e6:.0f}" for a, b, _ in spans))
    if spans and spans[0][2]:
        print("   messages:", " | ".join(m for _, _, m in spans[:4]))
    stats = {}
    for gname, idx in groups:
        sel = set(idx)
        ss = [s for s in samples if s[0] in sel]
        n = max(1, len(idx))
        durs = [(spans[i][1] - spans[i][0]) / 1e6 for i in idx]
        cpu = sum(s[2] for s in ss)
        med = statistics.median(durs) if durs else 0
        print(f"\n=== {label} / {gname}: opens {idx[:6]}{'…' if len(idx) > 6 else ''} (n={len(idx)})")
        print(f"   interval ms: median {med:.1f}  mean {statistics.mean(durs) if durs else 0:.1f}"
              f"  min {min(durs) if durs else 0:.1f}  max {max(durs) if durs else 0:.1f}")
        print(f"   main-thread CPU (sampled) per open: {cpu / n:.1f} ms = {100 * cpu / n / max(med, 1e-9):.0f}% of the median interval;"
              f" the rest the main thread was idle (waiting on frames, render server, timers)")
        tot = max(cpu, 1e-9)
        binself = collections.Counter()
        leaf = collections.Counter()
        app = collections.Counter()
        incl = collections.Counter()
        cats = collections.Counter()
        causes = collections.Counter()
        phases = collections.Counter()
        tree = {}
        for k, dt, w, fr in ss:
            causes[kind_of(fr)] += w
            phases[phase_of(fr)] += w
            binself[fr[0][1]] += w
            leaf[fr[0][0]] += w
            app[deepest_app(fr) or "(no Arbos frame on the stack: framework/run-loop work)"] += w
            for nm in set(n for n, _ in fr):
                incl[nm] += w
            names = [n for n, _ in fr]
            for cname, rx in CATEGORIES:
                if any(rx.search(nm) for nm in names):
                    cats[cname] += w
            node = tree
            for nm, _ in reversed(fr):
                ch = node.setdefault(nm, [0.0, {}])
                ch[0] += w
                node = ch[1]
        stats[gname] = (n, incl, cpu, causes, med, phases)

        def table(title, counter, k=15):
            print(f"   -- {title}")
            for key, v in counter.most_common(k):
                print(f"   {v / n:8.2f} ms {100 * v / tot:5.1f}%  {short(key)}")

        table("phase: which run-loop job (additive)", phases, 10)
        table("work kind (additive; named one-time causes first, else nearest-leaf kind)", causes, 22)
        table("self time by binary (additive)", binself, 12)
        table("time charged to the deepest Arbos frame on the stack (additive)", app, 20)
        table("self time by leaf frame (additive)", leaf, 20)
        print("   -- named categories (inclusive; a sample can count in several)")
        for cname, _ in CATEGORIES:
            v = cats[cname]
            print(f"   {v / n:8.2f} ms {100 * v / tot:5.1f}%  {cname}")
        print("   -- call tree, root to leaf (nodes ≥ 2.5% of this group's main-thread CPU; single-child chains joined by ›)")
        lines = [0]

        def walk(node, depth):
            for nm, (w, ch) in sorted(node.items(), key=lambda kv: -kv[1][0]):
                if w < 0.025 * tot or lines[0] > 140:
                    continue
                chain = [nm]
                cur = ch
                while True:
                    big = [(a, b) for a, b in cur.items() if b[0] >= 0.025 * tot]
                    if len(big) == 1 and big[0][1][0] >= 0.9 * w:
                        chain.append(big[0][0])
                        cur = big[0][1][1]
                    else:
                        break
                txt = chain[0] if len(chain) == 1 else f"{short(chain[0], 60)} › … › {short(chain[-1], 110)}" if len(chain) > 3 else " › ".join(short(c, 60) for c in chain)
                print(f"   {w / n:8.2f} ms {100 * w / tot:5.1f}% {'  ' * depth}{txt}")
                lines[0] += 1
                walk(cur, depth + 1)

        walk(tree, 0)
        if len(idx) == 1:
            print("   -- timeline of this open, 100 ms bins: CPU ms, then the top deepest-Arbos frames")
            bins = collections.defaultdict(collections.Counter)
            for k, dt, w, fr in ss:
                bins[int(dt / 1e8)][deepest_app(fr) or "(framework only)"] += w
            for b in sorted(bins):
                c = bins[b]
                tops = "; ".join(f"{short(x, 70)} {v:.0f}" for x, v in c.most_common(3))
                print(f"   {b * 100:5d}-{b * 100 + 100:<5d} {sum(c.values()):5.0f} ms  {tops}")
    names = list(stats)
    side_by_side(label, stats, names, 5, "phase")
    side_by_side(label, stats, names, 3, "work kind")
    return stats


def side_by_side(label, stats, names, col, what):
    print(f"\n=== {label}: {what}, ms of main-thread CPU per open, by group")
    print("   " + "".join(f"{g[:14]:>15s}" for g in names) + "  cause")
    print("   " + "".join(f"{stats[g][4]:>12.0f} ms" for g in names) + "  (median open interval)")
    print("   " + "".join(f"{stats[g][2] / stats[g][0]:>12.1f} ms" for g in names) + "  (main-thread CPU, all causes)")
    allc = collections.Counter()
    for g in names:
        allc.update({k: v / stats[g][0] for k, v in stats[g][col].items()})
    for cname, _ in allc.most_common():
        print("   " + "".join(f"{stats[g][col][cname] / stats[g][0]:>12.1f} ms" for g in names) + f"  {cname}")


def first_minus_repeat(label, stats):
    names = list(stats)
    for gname in names:
        if not gname.endswith("-first") and gname != "first":
            continue
        rname = "repeat" if gname == "first" else gname.replace("-first", "-repeat")
        if rname not in stats:
            continue
        n1, i1, c1 = stats[gname][:3]
        n2, i2, c2 = stats[rname][:3]
        diff = collections.Counter({k: i1[k] / n1 - i2.get(k, 0) / n2 for k in i1})
        gap = c1 / n1 - c2 / n2
        print(f"\n=== {label}: {gname} minus {rname}: main-thread CPU {c1 / n1:.1f} - {c2 / n2:.1f} = {gap:.1f} ms per open")
        print("   -- frames by extra inclusive ms on the first open (share of the CPU gap)")
        for k, v in diff.most_common(70):
            if v <= 0:
                break
            print(f"   {v:8.2f} ms {100 * v / max(gap, 1e-9):5.1f}%  {short(k)}")


def inner_intervals(spans, ivs, groups):
    """Other signpost intervals that start inside each open: count, ms, and offset from the open."""
    print("   -- signpost intervals starting inside the open (per open: count, total ms, first start offset ms)")
    for gname, idx in groups:
        rows = collections.defaultdict(list)
        for i in idx:
            a, b, _ = spans[i]
            for name, lst in ivs.items():
                if name == "open-chat":
                    continue
                inside = [(s, e) for s, e, _ in lst if a <= s <= b]
                if inside:
                    rows[name].append((len(inside), sum(e - s for s, e in inside) / 1e6, (inside[0][0] - a) / 1e6))
        print(f"   [{gname}]")
        for name, v in sorted(rows.items(), key=lambda kv: -statistics.median(x[1] for x in kv[1])):
            print(f"      {name:32s} in {len(v)}/{len(idx)} opens; median count {statistics.median(x[0] for x in v):.0f}"
                  f", total {statistics.median(x[1] for x in v):7.1f} ms, starts at +{statistics.median(x[2] for x in v):6.1f} ms")


def main():
    tp, sp, label = sys.argv[1:4]
    spec = sys.argv[4] if len(sys.argv) > 4 else "first=0;repeat=1-"
    if len(sys.argv) > 5:
        load_syms(sys.argv[5])
    ivs = intervals_of(sp)
    spans = ivs.get("open-chat", [])
    samples = samples_of(tp, spans)
    groups = parse_groups(spec, len(spans))
    stats = report(label, spans, samples, groups)
    first_minus_repeat(label, stats)
    print()
    inner_intervals(spans, ivs, groups)
    rb = ivs.get("rows-build", [])
    for i, (a, b, _) in enumerate(spans[:3]):
        inside = [((s - a) / 1e6, (e - s) / 1e6, m) for s, e, m in rb if a <= s <= b]
        print(f"   open {i}: rows-build " + "; ".join(f"+{o:.0f} ms for {d:.1f} ms [{m}]" for o, d, m in inside))


if __name__ == "__main__":
    main()
