# Connecting and thinking are the same picture, and thinking is the long one

Filed at cycle 196. Measured, reviewed, and not guessed at.

The call screen is wordless on purpose: the orb's colour and motion carry the
state. Two of the four states are drawn identically, and one of them is where
a person spends most of the call.

## What was measured

Stills labelled by the accessibility phase, orb colour averaged over a disc
inside the orb:

| phase | orb colour | pulse swing |
| --- | --- | --- |
| connecting | (174, 174, 174) | 4.5% |
| listening | (222, 222, 222) | 8.5% |
| thinking | **(174, 174, 174)** | **4.5%** |
| speaking | (125, 162, 212) | 5.9% |

Connecting and thinking differ by **zero** in every channel. The pulse swings
were measured from the film at 5 frames a second, across the orb's widest
row.

## Why, in the code

Both switches in `VoiceOrb` group them on purpose:

```swift
case .thinking, .connecting: return 0.90 + 0.04 * CGFloat(pulse)   // scale
case .thinking, .connecting: return ArbosTheme.textMuted           // colour
```

So this is a decision, not an oversight. The 4% coefficient is exactly the
4.5% measured off the film, which is a pleasant confirmation that the
measurement is reading the right thing.

## What a viewer sees

An independent review of the film, asked specifically whether motion
separates the states:

> **Connecting (00:06–00:07):** completely static.
> **Listening (00:07–00:14):** breathes and pulses with a soft outer glow.
> **Thinking (00:14–00:23):** completely static. Zero motion.
> **Speaking (00:23–00:30):** breathes again, in blue.
>
> There is absolutely no visible difference between the early connecting
> period and the long middle thinking period.
>
> Because the thinking state is completely static and identical to the
> connecting state, the app visually appears as though it has frozen,
> stalled, or dropped the connection during this 9-second window.

The pulse is there — 4.5% is not nothing — but at that amplitude, watched by
someone told to look for it, it reads as a still picture.

## Why it matters more than it looks

Thinking is the longest part of a call. Measured across four runs: 7.4, 7.8,
9.4 and 9.8 seconds, against about 2 seconds of connecting. So the state that
holds the screen longest is the one drawn identically to the briefest, and
the identity under the orb never changes either — cycle 193 found the screen
carries no words at any point.

The reviewer also noted every transition is instantaneous, with no easing,
though `.animation(.easeInOut(duration: 0.35), value: phase)` is on the view.
Worth a look if this is taken up.

## The decision wanted

Should "waiting for the kernel" look different from "opening the line"? If
yes, the smallest honest change is a distinct treatment for `.thinking` —
either its own colour or a pulse nearer listening's 8.5%. If no, then the
9-second frozen-looking window is accepted on purpose, and the next person to
measure it should find that written down rather than rediscover it.

Not changed here: which way it should go is a design call, and this loop does
not ship product guesses.
