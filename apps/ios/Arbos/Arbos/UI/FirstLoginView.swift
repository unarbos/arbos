// D82: the first-login screen. A terminal panel types muted token bars behind a
// prompt while the relay builds the user's computer and first agent; the hero
// text underneath says what is happening (real relay stages, see
// Store/FirstLogin.swift). On finish the panel morphs into the agent's greeting
// bubble at the exact frame the chat draws it, and the live chat fades in
// underneath. Native shapes only — no Lottie runtime (D79 removed it).

import SwiftUI

struct FirstLoginView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.theme) private var theme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// The state at creation; the body reads the live value from the model so
    /// stage changes land inside the timeline closure on the next frame.
    let initial: FirstLogin

    init(state: FirstLogin) {
        initial = state
    }

    private var state: FirstLogin { model.firstLogin ?? initial }

    private static let cardSize = CGSize(width: 256, height: 172)
    private static let cardRadius: CGFloat = 22

    var body: some View {
        GeometryReader { geo in
            let size = geo.size
            let cardCenter = CGPoint(x: size.width / 2, y: size.height * 0.40)
            TimelineView(.animation(minimumInterval: 1.0 / 60.0, paused: reduceMotion && state.stage != .finishing)) { timeline in
                let state = self.state
                let target = bubbleTarget(in: size)
                let now = timeline.date
                let p = state.finishProgress(at: now)
                let fin = p.map { Terminal.ease($0) }
                ZStack {
                    theme.bg
                        .opacity(fin.map { 1 - Terminal.ramp($0, 0.6, 1.0) } ?? 1)
                        .ignoresSafeArea()
                    terminal(state: state, now: now, p: fin, cardCenter: cardCenter, target: target)
                    if state.stage != .finishing {
                        textBlock(state: state, cardCenter: cardCenter)
                            .frame(width: size.width)
                            .position(x: size.width / 2, y: cardCenter.y + Self.cardSize.height / 2 + 72)
                    }
                }
            }
        }
        .ignoresSafeArea()
        .accessibilityIdentifier("first-login")
    }

    // MARK: Terminal

    private func terminal(state: FirstLogin, now: Date, p: Double?, cardCenter: CGPoint, target: CGRect) -> some View {
        let t = reduceMotion ? 2.0 : now.timeIntervalSince(state.startedAt)
        let loopRect = CGRect(x: cardCenter.x - Self.cardSize.width / 2, y: cardCenter.y - Self.cardSize.height / 2,
                              width: Self.cardSize.width, height: Self.cardSize.height)
        // Reduce Motion: no morph — the static panel and the page simply fade out
        // over the live chat.
        let morph = reduceMotion ? 0 : (p.map { Terminal.ramp($0, 0.15, 0.5) } ?? 0)
        let rect = Terminal.lerp(loopRect, target, morph)
        let radius = Self.cardRadius + (Theme.bubbleRadius - Self.cardRadius) * morph
        let contentAlpha = reduceMotion ? 1 : (p.map { 1 - Terminal.ramp($0, 0, 0.25) } ?? 1)
        let greetingAlpha = reduceMotion ? 0 : (p.map { Terminal.ramp($0, 0.5, 0.7) } ?? 0)
        let wholeAlpha = reduceMotion ? (p.map { 1 - $0 } ?? 1) : 1
        return ZStack(alignment: .topLeading) {
            Canvas { ctx, _ in
                let path = Path(roundedRect: rect, cornerRadius: radius, style: .continuous)
                if morph < 1 {
                    // Soft shadow under the panel (fades as it becomes a bubble).
                    var shadow = ctx
                    shadow.opacity = (theme.isDark ? 0.30 : 0.05) * (1 - morph)
                    shadow.fill(Path(roundedRect: rect.insetBy(dx: 8, dy: 4).offsetBy(dx: 0, dy: 8),
                                     cornerRadius: radius, style: .continuous), with: .color(.black))
                }
                ctx.fill(path, with: .color(theme.bubbleAssistant))
                if contentAlpha > 0 {
                    var inner = ctx
                    inner.opacity = contentAlpha
                    inner.clip(to: Path(roundedRect: loopRect.insetBy(dx: 4, dy: 4), cornerRadius: radius - 4, style: .continuous))
                    Terminal.draw(in: &inner, card: loopRect, t: t, ink: theme.text, dark: theme.isDark, frozen: reduceMotion)
                }
            }
            if greetingAlpha > 0 {
                Text(greetingText(state))
                    .font(theme.font(Theme.body))
                    .lineSpacing(Theme.bodyLineSpacing)
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, Theme.bubbleHPad)
                    .padding(.vertical, Theme.bubbleVPad)
                    .frame(width: target.width, height: target.height, alignment: .topLeading)
                    .offset(x: target.minX, y: target.minY)
                    .opacity(greetingAlpha)
            }
        }
        .opacity(wholeAlpha)
    }

    private func greetingText(_ state: FirstLogin) -> String {
        "Hi, I’m \(state.agentName). Your computer is ready — what should we build first?"
    }

    /// Where the greeting bubble sits in the chat underneath: the frame the
    /// transcript reported, else Grok's first-bubble geometry on this screen.
    private func bubbleTarget(in size: CGSize) -> CGRect {
        if let f = model.greetingBubbleFrame, f.width > 40, f.height > 20 { return f }
        let width = min(size.width - Theme.bubbleInset, 330)
        let top = (UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first?.safeAreaInsets.top ?? 59)
            + 64 + 36
        return CGRect(x: 16, y: top, width: width, height: 88)
    }

    // MARK: Hero text / error block

    @ViewBuilder
    private func textBlock(state: FirstLogin, cardCenter: CGPoint) -> some View {
        if state.stage == .failed {
            VStack(spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: "desktopcomputer").font(.system(size: 14))
                    Text(state.title)
                }
                .font(theme.font(Theme.body, weight: .semibold))
                .foregroundStyle(theme.text)
                if let error = state.error {
                    Text(error)
                        .font(theme.font(Theme.small))
                        .foregroundStyle(theme.muted)
                        .multilineTextAlignment(.center)
                }
                Button { model.retryFirstLogin() } label: {
                    Text("Retry")
                        .font(theme.font(Theme.small, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .padding(.horizontal, 18)
                        .padding(.vertical, 9)
                        .background(Capsule().fill(theme.field)) // control-size: exception inlineChips
                        .overlay(Capsule().strokeBorder(theme.fieldBorder, lineWidth: 1)) // control-size: exception inlineChips
                }
                .buttonStyle(.plain)
                .padding(.top, 8)
            }
            .padding(.horizontal, 32)
            .transition(.opacity)
        } else {
            VStack(spacing: 8) {
                Text(state.title)
                    .font(theme.font(22, weight: .semibold))
                    .foregroundStyle(theme.text)
                    .id("title-\(state.title)")
                    .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: 6)),
                                            removal: .opacity.combined(with: .offset(y: -6))))
                Text(state.detail ?? " ")
                    .font(theme.font(Theme.small))
                    .foregroundStyle(theme.muted)
                    .multilineTextAlignment(.center)
                    .contentTransition(.opacity)
                    .lineLimit(2)
            }
            .padding(.horizontal, 32)
            .animation(reduceMotion ? nil : .easeInOut(duration: 0.35), value: state.title)
            .animation(reduceMotion ? nil : .easeInOut(duration: 0.3), value: state.detail)
        }
    }
}

/// The terminal's geometry and timeline (the same numbers as the approved
/// Lottie concept `terminal-boot`: a 4 s loop, six lines, one new line every
/// 0.667 s, 24 pt pitch, linear typing, fade at the top).
enum Terminal {
    static let loop = FirstLogin.loopSeconds
    static let lines = 6
    static let perLine = loop / Double(lines)
    static let slide = 14.0 / 60.0
    static let typeStart = 14.0 / 60.0
    static let typeEnd = 38.0 / 60.0
    static let fadeStart = 200.0 / 60.0
    static let fadeEnd = 232.0 / 60.0
    static let pitch: CGFloat = 24
    static let barHeight: CGFloat = 8

    /// (offset, width, alpha) per bar; six "token" lines of different shapes.
    static let patterns: [[(CGFloat, CGFloat, Double)]] = [
        [(0, 44, 0.42), (52, 78, 0.20)],
        [(0, 30, 0.42), (38, 110, 0.20)],
        [(0, 56, 0.42), (64, 40, 0.20), (112, 52, 0.20)],
        [(0, 38, 0.42), (46, 92, 0.20)],
        [(0, 64, 0.42), (72, 66, 0.20)],
        [(0, 26, 0.42), (34, 124, 0.20)],
    ]

    static func slotY(_ k: Int) -> CGFloat { 52 - pitch * CGFloat(k) }

    static func draw(in ctx: inout GraphicsContext, card: CGRect, t: Double, ink: Color, dark: Bool, frozen: Bool) {
        let cx = card.midX, cy = card.midY
        let x0 = -card.width / 2 + 22
        let barX = x0 + 18
        let alphaScale = dark ? 1.15 : 1.0

        for i in 0..<lines {
            let tau = (t + Double(i) * perLine).truncatingRemainder(dividingBy: loop)
            let k = min(lines - 1, Int(tau / perLine))
            var y = slotY(k)
            if k >= 1 {
                let s = (tau - Double(k) * perLine) / slide
                if s < 1 { y = slotY(k - 1) + (slotY(k) - slotY(k - 1)) * CGFloat(ease(max(0, s))) }
            }
            var alpha = 1.0
            if tau > fadeStart { alpha = 1 - smooth(min(1, (tau - fadeStart) / (fadeEnd - fadeStart))) }
            if alpha <= 0 { continue }
            let segs = patterns[i]
            let total = segs.last.map { $0.0 + $0.1 } ?? 1
            // Typing is linear in characters; a frozen (Reduce Motion) panel shows full lines.
            let typed: CGFloat
            if frozen || k > 0 {
                typed = total
            } else if tau <= typeStart {
                typed = 0
            } else {
                typed = total * CGFloat(min(1, (tau - typeStart) / (typeEnd - typeStart)))
            }
            for (off, w, a) in segs {
                let visible = min(w, max(0, typed - off))
                if visible <= 0 { continue }
                let r = CGRect(x: cx + barX + off, y: cy + y - barHeight / 2, width: visible, height: barHeight)
                ctx.fill(Path(roundedRect: r, cornerRadius: 4), with: .color(ink.opacity(a * alphaScale * alpha)))
            }
        }

        // Prompt chevron on the active slot.
        var chevron = Path()
        chevron.move(to: CGPoint(x: cx + x0 + 2, y: cy + slotY(0) - 5))
        chevron.addLine(to: CGPoint(x: cx + x0 + 7, y: cy + slotY(0)))
        chevron.addLine(to: CGPoint(x: cx + x0 + 2, y: cy + slotY(0) + 5))
        ctx.stroke(chevron, with: .color(ink.opacity(dark ? 0.55 : 0.45)),
                   style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))

        // Block cursor: follows the active line's typing, blinks twice a second.
        let activeIndex = ((lines - Int(t / perLine) % lines) % lines)
        let tauA = t.truncatingRemainder(dividingBy: perLine)
        let segsA = patterns[activeIndex]
        let totalA = segsA.last.map { $0.0 + $0.1 } ?? 0
        let progress: CGFloat = frozen ? 1 : (tauA <= typeStart ? 0 : CGFloat(min(1, (tauA - typeStart) / (typeEnd - typeStart))))
        let cursorX = cx + barX + totalA * progress + 6 * progress + 4
        let on = frozen || Int(t * 2) % 2 == 0
        if on {
            let r = CGRect(x: cursorX, y: cy + slotY(0) - 7, width: 8, height: 14)
            ctx.fill(Path(roundedRect: r, cornerRadius: 2), with: .color(ink.opacity(dark ? 0.85 : 0.7)))
        }
    }

    // MARK: Easing helpers

    /// Material standard curve, cubic-bezier(0.4, 0, 0.2, 1).
    static func ease(_ x: Double) -> Double {
        bezier(min(1, max(0, x)), 0.4, 0.0, 0.2, 1.0)
    }

    static func smooth(_ p: Double) -> Double { p * p * (3 - 2 * p) }

    /// 0 before `a`, 1 after `b`, eased in between.
    static func ramp(_ p: Double, _ a: Double, _ b: Double) -> Double {
        smooth(min(1, max(0, (p - a) / (b - a))))
    }

    static func lerp(_ a: CGRect, _ b: CGRect, _ t: Double) -> CGRect {
        let t = CGFloat(t)
        return CGRect(x: a.minX + (b.minX - a.minX) * t, y: a.minY + (b.minY - a.minY) * t,
                      width: a.width + (b.width - a.width) * t, height: a.height + (b.height - a.height) * t)
    }

    private static func bezier(_ x: Double, _ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double) -> Double {
        var t = x
        for _ in 0..<10 {
            let cx = 3 * x1 * (1 - t) * (1 - t) * t + 3 * x2 * (1 - t) * t * t + t * t * t
            let dx = 3 * (1 - t) * (1 - t) * x1 + 6 * (1 - t) * t * (x2 - x1) + 3 * t * t * (1 - x2)
            if abs(dx) < 1e-6 { break }
            t = min(1, max(0, t - (cx - x) / dx))
        }
        return 3 * y1 * (1 - t) * (1 - t) * t + 3 * y2 * (1 - t) * t * t + t * t * t
    }
}
