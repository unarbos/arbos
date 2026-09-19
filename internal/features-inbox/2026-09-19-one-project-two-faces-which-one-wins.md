---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# One project, two faces — which one wins?

The same project is drawn in two different colours depending on which screen
you are looking at. Measured on `phone`:

| screen | glyph |
|---|---|
| projects list | `(118, 93, 209)` — purple |
| call screen | `(229, 83, 61)` — `0xE5533D`, the palette entry named **red** |

Both are "correct" by their own route. They take the project's face from
different places.

## Where each one comes from

**The list** uses the hub roster. `ProjectStore` applies it like this:

    if let face = project.identity?.filled(key: target.stored) {
        row.identity = face
        remember(face, for: target)
    }

`filled(key:)` substitutes a colour **only when the roster's is empty**, and
the substitute is derived by hashing `target.stored` — the string
`hub:<machine>/<project>`. For `phone` that hash lands on purple.

**The chat and the call screen** use the project's own kernel. `ChatStore`
takes the identity straight off the `hello` frame:

    case .identity(let face):
        identity = face

with no `filled(key:)` and no hashing. The kernel says red, so red is drawn.

That reading was wrong, and measuring a second project showed why.

## Three projects, and the call screen cannot tell two of them apart

| project | list glyph | call glyph |
|---|---|---|
| `phone` | purple `(118, 93, 209)` | `0xE5533D` red |
| `demo` | teal | `0xE5533D` red — **the same** |
| `const` | orange | `0x4C8DFF` blue — the palette's **first** entry |

Two different projects draw the identical face under the orb. A third draws
`colors[0]`, which is what `tint` returns when it does not recognise a colour
at all.

So the call screen is not drawing the *project's* face. It is drawing
whatever identity the attached **kernel** sent — and one kernel serves
several projects, so those projects share a face. Where no identity arrived,
it falls through to the first colour in the palette.

The list, which hashes each project's own key, is the one telling them apart.
The call screen's face identifies the kernel you are talking to, or nothing.

## It is not only the call screen (cycle 181)

Walking the app for the purpose check, the **chat header** draws it too.
Measured on the same build, same project:

| screen | glyph |
|---|---|
| projects list | `0x9A7AFE` purple |
| chat header | `0xE5533D` red |
| call screen | `0xE5533D` red |

The chat header is the screen you see every time you open a project. The
call screen is not. This matters more than it looked when it was filed.

## The cause is one line, and it was written on purpose

Cycle 166 said the fix needed two halves and that the first was missing —
the chat writing the kernel's face into the cache the list reads. **It is not
missing.** `MainChatView` already does it:

    .onChange(of: chat.identity) { _, face in
        if let face { projects.remember(face, for: target) }
    }

So the list is told. It is then untold, on the next roster refresh, by
`ProjectStore`:

    // The roster's face (#233) beats the cache and the default.
    if let face = project.identity?.filled(key: target.stored) {
        row.identity = face
        remember(face, for: target)
    }

`filled(key:)` substitutes a colour **only when the roster's is empty**, and
what it substitutes is the hash of `hub:<machine>/<project>`. So a roster
entry that carries no colour of its own still overwrites the colour the
project's own kernel supplied — and overwrites the remembered copy with it,
so the next cold start reads the hash too.

That is one line, and its comment says it is deliberate: #233 decided the
roster wins.

## Why this is a decision and not a patch

The obvious repair — have the chat remember what the kernel said, so the list
can use it — does not hold on its own. The roster face is re-applied on every
refresh, and a roster face with no colour is filled from the hash again, so
the remembered colour would be overwritten within seconds.

Making it stick needs both halves:

1. the chat writes the kernel's face into the same cache the list reads
   (`identity:<target.stored>` in defaults), and
2. the roster stops overwriting a colour learned from a kernel with one
   invented from a hash.

That is a change across two stores, and it rests on a choice nobody has
made yet:

**Should a roster face with no colour of its own beat a colour the project's
own kernel supplied?**

That is the whole question now, and it is one line. #233 made the roster win
over "the cache and the default" — and when the roster's colour is empty,
what wins is not the roster, it is a hash. The kernel's answer is the
project's configured identity, the same one the desktop uses.

The old framing, kept because it is the same decision seen from the orb:

It currently draws the kernel's, which means two projects on one kernel are
indistinguishable there, and a project whose kernel sent no identity is drawn
in the palette's first colour rather than its own. If the answer is "the
project's", the call screen should fall back to the same per-project key the
list uses rather than to `colors[0]`, and should prefer the project key over
a kernel identity that is not specific to this project.

If the answer is "the kernel's", then the two screens are answering different
questions and that is fine — but the orb should probably not be using the
same glyph-and-colour vocabulary the list uses for projects, because it reads
as the same fact and is not.

## What is measured

- `media/mobile/cycle-165/01-the-call.png` — `phone`'s call screen, red glyph
- `media/mobile/cycle-166/01-demo.png` — `demo`'s, the same red
- `media/mobile/cycle-166/01-const.png` — `const`'s, the palette's first blue
- `media/mobile/cycle-163/01-the-dot-is-drawn.png` — the list, purple glyph
- cycle 165 also tried fixing the call screen's *fallback* key
  (`<machine>/<project>` against `hub:<machine>/<project>`). That mismatch is
  real but is not this: fixing it moved nothing, because the fallback is not
  the path taken when the kernel supplies a face. The change was reverted.

Filed by the mobile loop at cycle 166. No app change made.
