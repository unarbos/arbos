import SwiftUI
import Charts
import Photos
import ArbosFFI

// Chart bubbles ([feedback chart-bubble-ios-2026-10-26]): the card an agent's `chart` part
// draws inside its bubble — the code card's frame (radius 12, the card fill, a hairline, the
// 24 pt icon squares top-right: Copy data · Save image · Expand), title and subtitle, the legend,
// and Swift Charts marks fed from `arbos_core::chart`'s resolved model through the FFI
// (`chartModelJson`): the same domains, ticks, formatted values and readout columns the Mac and
// the fallback picture use. Touch-and-hold shows the nearest column's values; Expand opens a
// full-screen cover with the plot at the screen's width, a Chart | Data switch and the two
// pills; Save image renders the card through `ImageRenderer` at 3x into Photos.

// MARK: - The resolved model (Codable twin of `arbos_core::chart::ChartModel`)

struct ChartModel: Codable, Equatable {
    struct Tick: Codable, Equatable {
        var value: Double
        var label: String
    }
    struct Axis: Codable, Equatable {
        var label: String?
        var unit: String
        var scale: String
        var format: String
        var min: Double
        var max: Double
        var ticks: [Tick]
        var categories: [String]? = nil
    }
    struct Point: Codable, Equatable {
        var x: Double
        var y: Double?
        var y0: Double
    }
    struct Series: Codable, Equatable {
        var name: String
        var colorIndex: Int
        var color: String?
        var style: String
        var axis: String
        var points: [Point]
    }
    struct Annotation: Codable, Equatable {
        var kind: String
        var x: Double?
        var y: Double?
        var from: Double?
        var to: Double?
        var label: String?
    }
    struct ColumnEntry: Codable, Equatable {
        var series: Int
        var value: Double
        var text: String
    }
    struct Column: Codable, Equatable {
        var x: Double
        var label: String
        var entries: [ColumnEntry]
    }

    var kind: String
    var title: String?
    var subtitle: String?
    var series: [Series]
    var categories: [String]
    var x: Axis
    var y: Axis
    var y2: Axis?
    var stacked: Bool
    var showLegend: Bool
    var legendBottom: Bool
    var annotations: [Annotation]
    var columns: [Column]
    var shares: [Double]? = nil

    var isPie: Bool { kind == "pie" }
    var isBar: Bool { kind == "bar" }
    var isCategorical: Bool { isBar || isPie }

    /// The nearest readout column to a data-space x, within one column step (the core's
    /// `column_near`).
    func columnNear(_ x: Double) -> Int? {
        guard !columns.isEmpty else { return nil }
        var best = 0
        for (i, c) in columns.enumerated() where abs(c.x - x) < abs(columns[best].x - x) { best = i }
        let step = columns.count > 1 ? max(abs(columns[1].x - columns[0].x), 1e-9) : Double.infinity
        return abs(columns[best].x - x) <= step ? best : nil
    }

    /// Decode the FFI's JSON (snake_case keys).
    static func decode(_ json: String) -> ChartModel? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(ChartModel.self, from: Data(json.utf8))
    }
}

/// Parsed charts per spec: the model is resolved once per spec string (hover and scrolling
/// re-render the view, not the numbers).
@MainActor
enum ChartModelCache {
    private static var models: [String: ChartModel] = [:]
    private static var order: [String] = []

    static func model(for spec: String) -> ChartModel? {
        if let m = models[spec] { return m }
        guard let json = try? chartModelJson(specJson: spec), let m = ChartModel.decode(json) else { return nil }
        if order.count >= 48, let oldest = order.first {
            models.removeValue(forKey: oldest)
            order.removeFirst()
        }
        models[spec] = m
        order.append(spec)
        return m
    }

    /// "Copy data": the TSV with a header row, from the core.
    static func tsv(for spec: String) -> String? {
        try? chartTableTsv(specJson: spec)
    }
}

// MARK: - Palette (the code-card token colours per theme; Mac `chart_card::palette`)

enum ChartPalette {
    static let darkColors: [Color] = [
        Color(red: 0x95 / 255, green: 0xD0 / 255, blue: 0xCD / 255),
        Color(red: 0xE6 / 255, green: 0xB3 / 255, blue: 0x87 / 255),
        Color(red: 0xD8 / 255, green: 0x98 / 255, blue: 0xD8 / 255),
        Color(red: 0xF1 / 255, green: 0xC9 / 255, blue: 0x72 / 255),
        Color(red: 0x10 / 255, green: 0x84 / 255, blue: 0xFE / 255),
        Color(red: 0x34 / 255, green: 0xC7 / 255, blue: 0x59 / 255),
    ]
    static let lightColors: [Color] = [
        Color(red: 0x1F / 255, green: 0x7F / 255, blue: 0x7A / 255),
        Color(red: 0xB8 / 255, green: 0x64 / 255, blue: 0x1A / 255),
        Color(red: 0xA6 / 255, green: 0x3F / 255, blue: 0xA4 / 255),
        Color(red: 0x9A / 255, green: 0x6F / 255, blue: 0x00 / 255),
        Color(red: 0x0A / 255, green: 0x6F / 255, blue: 0xE0 / 255),
        Color(red: 0x24 / 255, green: 0x8A / 255, blue: 0x3D / 255),
    ]

    /// `#RRGGBB` → colour, or nil.
    static func hex(_ s: String?) -> Color? {
        guard let s, s.hasPrefix("#"), s.count == 7, let v = UInt32(s.dropFirst(), radix: 16) else { return nil }
        return Color(red: Double((v >> 16) & 0xff) / 255, green: Double((v >> 8) & 0xff) / 255, blue: Double(v & 0xff) / 255)
    }

    /// The colour of series `i` (its own, or the theme palette's).
    static func series(_ model: ChartModel, _ i: Int, dark: Bool) -> Color {
        if i < model.series.count, let own = hex(model.series[i].color) { return own }
        let p = dark ? darkColors : lightColors
        return p[i % p.count]
    }

    /// The colour of pie slice `i`.
    static func slice(_ model: ChartModel, _ i: Int, dark: Bool) -> Color {
        if let own = hex(model.series.first?.color) { return own }
        let p = dark ? darkColors : lightColors
        return p[i % p.count]
    }
}

// MARK: - Geometry

enum ChartCardMetrics {
    static let radius: CGFloat = 12
    static let pad: CGFloat = 12
    static let iconSquare: CGFloat = 24
    static let iconGlyph: CGFloat = 13
    static let plotHeight: CGFloat = 200
    static let barPlotHeight: CGFloat = 190
    static let piePlotHeight: CGFloat = 210
    static let copiedFor: Duration = .seconds(1.5)

    static func height(kind: String) -> CGFloat {
        switch kind {
        case "bar": return barPlotHeight
        case "pie": return piePlotHeight
        default: return plotHeight
        }
    }
}

// MARK: - The card

struct ChartCard: View {
    @Environment(\.theme) private var theme
    let partId: String
    let spec: String
    @State private var copied = false
    @State private var saved: String? = nil
    @State private var expanded = false

    var body: some View {
        if let model = ChartModelCache.model(for: spec) {
            card(model)
        } else {
            Text("This chart needs a newer version of Arbos.")
                .font(theme.font(Theme.small))
                .foregroundStyle(theme.muted)
                .padding(ChartCardMetrics.pad)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: ChartCardMetrics.radius, style: .continuous).fill(theme.bg.opacity(0.6)))
                .accessibilityIdentifier("chart-unreadable")
        }
    }

    @ViewBuilder
    private func card(_ model: ChartModel) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack(alignment: .topTrailing) {
                VStack(alignment: .leading, spacing: 2) {
                    if let t = model.title, !t.trimmingCharacters(in: .whitespaces).isEmpty {
                        Text(t)
                            .font(theme.font(Theme.small, weight: .semibold))
                            .foregroundStyle(theme.text)
                            .accessibilityIdentifier("chart-title")
                    }
                    if let s = model.subtitle, !s.isEmpty {
                        Text(s)
                            .font(theme.font(Theme.tiny))
                            .foregroundStyle(theme.muted)
                    }
                }
                .padding(.trailing, 100)
                .frame(maxWidth: .infinity, alignment: .leading)
                ChartIconSquares(
                    copied: copied, saved: saved != nil,
                    onCopy: copyData, onSave: { saveImage(model) }, onExpand: { expanded = true }
                )
                .offset(x: 4, y: -4)
            }
            if model.showLegend {
                ChartLegend(model: model, font: Theme.tiny, dot: 8)
                    .padding(.top, 10)
                    .padding(.bottom, 2)
            }
            ChartPlot(model: model, font: 12, lineWidth: 2.2)
                .frame(height: ChartCardMetrics.height(kind: model.kind))
                .accessibilityIdentifier("chart-plot")
            if let saved {
                Text(saved)
                    .font(theme.font(Theme.tiny))
                    .foregroundStyle(theme.muted)
                    .padding(.top, 4)
                    .transition(.opacity)
            }
        }
        .padding(ChartCardMetrics.pad)
        .padding(.bottom, -2)
        .background(RoundedRectangle(cornerRadius: ChartCardMetrics.radius, style: .continuous).fill(theme.bg.opacity(0.6)))
        .overlay(RoundedRectangle(cornerRadius: ChartCardMetrics.radius, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .clipShape(RoundedRectangle(cornerRadius: ChartCardMetrics.radius, style: .continuous))
        .accessibilityIdentifier("chart-card")
        .fullScreenCover(isPresented: $expanded) {
            ChartViewer(spec: spec, model: model, onClose: { expanded = false })
        }
    }

    private func copyData() {
        guard let tsv = ChartModelCache.tsv(for: spec) else { return }
        UIPasteboard.general.string = tsv
        Haptics.pick()
        copied = true
        Task { @MainActor in
            try? await Task.sleep(for: ChartCardMetrics.copiedFor)
            copied = false
        }
    }

    private func saveImage(_ model: ChartModel) {
        Task { @MainActor in
            let ok = await ChartImage.saveToPhotos(model: model, theme: theme)
            saved = ok ? "Saved to Photos" : "Couldn't save to Photos"
            Haptics.pick()
            try? await Task.sleep(for: .seconds(1.8))
            saved = nil
        }
    }
}

/// The three 24 pt squares (the code card's copy square, three times).
struct ChartIconSquares: View {
    @Environment(\.theme) private var theme
    let copied: Bool
    let saved: Bool
    let onCopy: () -> Void
    let onSave: () -> Void
    let onExpand: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            square(copied ? "checkmark" : "doc.on.doc", tint: copied ? theme.online : nil, id: "chart-copy", label: copied ? "Copied" : "Copy data", action: onCopy)
            square(saved ? "checkmark" : "square.and.arrow.down", tint: saved ? theme.online : nil, id: "chart-save", label: "Save image", action: onSave)
            square("arrow.up.left.and.arrow.down.right", tint: nil, id: "chart-expand", label: "Expand", action: onExpand)
        }
    }

    private func square(_ symbol: String, tint: Color?, id: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: ChartCardMetrics.iconGlyph, weight: .medium))
                .foregroundStyle(tint ?? theme.muted)
                .frame(width: ChartCardMetrics.iconSquare, height: ChartCardMetrics.iconSquare)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
        .accessibilityLabel(label)
    }
}

/// A dot per series (or per slice) and its name.
struct ChartLegend: View {
    @Environment(\.theme) private var theme
    let model: ChartModel
    let font: CGFloat
    let dot: CGFloat

    private var items: [(String, Color)] {
        if model.isPie {
            return model.categories.enumerated().map { ($1, ChartPalette.slice(model, $0, dark: theme.isDark)) }
        }
        return model.series.enumerated().map { ($1.name, ChartPalette.series(model, $0, dark: theme.isDark)) }
    }

    var body: some View {
        FlowRow(spacing: 14, lineSpacing: 4) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                HStack(spacing: 6) {
                    Circle().fill(item.1).frame(width: dot, height: dot)
                    Text(item.0).font(theme.font(font)).foregroundStyle(theme.muted)
                }
            }
        }
        .accessibilityIdentifier("chart-legend")
    }
}

/// A left-aligned wrapping row (the legend wraps when the names do not fit).
struct FlowRow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, lineH: CGFloat = 0, maxW: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += lineH + lineSpacing
                lineH = 0
            }
            x += size.width + spacing
            lineH = max(lineH, size.height)
            maxW = max(maxW, x - spacing)
        }
        return CGSize(width: width == .infinity ? maxW : width, height: y + lineH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x: CGFloat = bounds.minX, y: CGFloat = bounds.minY, lineH: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += lineH + lineSpacing
                lineH = 0
            }
            s.place(at: CGPoint(x: x, y: y), proposal: .unspecified)
            x += size.width + spacing
            lineH = max(lineH, size.height)
        }
    }
}

// MARK: - The plot (Swift Charts from the core model)

struct ChartPlot: View {
    @Environment(\.theme) private var theme
    let model: ChartModel
    var font: CGFloat = 12
    var lineWidth: CGFloat = 2.2
    /// The selected column (touch-and-hold), by index.
    @State private var selected: Int? = nil

    private func color(_ i: Int) -> Color { ChartPalette.series(model, i, dark: theme.isDark) }

    var body: some View {
        if model.isPie {
            pie
        } else {
            xy
        }
    }

    // ----- line / area / scatter / bar -----

    private var xy: some View {
        Chart {
            ForEach(Array(model.annotations.enumerated()), id: \.offset) { _, a in
                switch a.kind {
                case "band":
                    if let from = a.from, let to = a.to {
                        RectangleMark(xStart: .value("from", from), xEnd: .value("to", to))
                            .foregroundStyle(theme.text.opacity(0.05))
                            .annotation(position: .overlay, alignment: .topLeading) { annotationLabel(a.label) }
                    }
                case "vline":
                    if let x = a.x {
                        RuleMark(x: .value("x", x))
                            .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                            .foregroundStyle(theme.muted)
                            .annotation(position: .trailing, alignment: .top) { annotationLabel(a.label) }
                    }
                case "hline":
                    if let y = a.y {
                        RuleMark(y: .value("y", y))
                            .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                            .foregroundStyle(theme.muted)
                            .annotation(position: .top, alignment: .trailing) { annotationLabel(a.label) }
                    }
                default:
                    if let x = a.x, let y = a.y {
                        PointMark(x: .value("x", x), y: .value("y", y))
                            .symbol(.circle).symbolSize(60)
                            .foregroundStyle(theme.text)
                            .annotation(position: .topTrailing) { annotationLabel(a.label) }
                    }
                }
            }
            ForEach(Array(model.series.enumerated()), id: \.offset) { si, s in
                ForEach(Array(s.points.enumerated()), id: \.offset) { _, p in
                    if let y = p.y {
                        switch model.kind {
                        case "bar" where model.stacked:
                            BarMark(x: .value("x", p.x), yStart: .value("y0", p.y0), yEnd: .value("y", y), width: .ratio(0.62))
                                .foregroundStyle(color(si).opacity(selected == nil || selected == model.columnNear(p.x) ? 1 : 0.45))
                                .clipShape(UnevenRoundedRectangle(topLeadingRadius: 3, topTrailingRadius: 3))
                        case "bar":
                            BarMark(x: .value("x", p.x), yStart: .value("y0", p.y0), yEnd: .value("y", y), width: .ratio(0.62))
                                .position(by: .value("series", s.name))
                                .foregroundStyle(color(si).opacity(selected == nil || selected == model.columnNear(p.x) ? 1 : 0.45))
                                .clipShape(UnevenRoundedRectangle(topLeadingRadius: 3, topTrailingRadius: 3))
                        case "scatter":
                            PointMark(x: .value("x", p.x), y: .value("y", y))
                                .symbolSize(lineWidth * lineWidth * 9)
                                .foregroundStyle(color(si).opacity(0.9))
                        case "area":
                            AreaMark(x: .value("x", p.x), yStart: .value("y0", model.stacked ? p.y0 : model.y.min), yEnd: .value("y", y), series: .value("series", s.name))
                                .interpolationMethod(.linear)
                                .foregroundStyle(color(si).opacity(0.18))
                            LineMark(x: .value("x", p.x), y: .value("y", y), series: .value("series", s.name))
                                .interpolationMethod(.linear)
                                .lineStyle(lineStyle(s.style))
                                .foregroundStyle(color(si))
                        default:
                            LineMark(x: .value("x", p.x), y: .value("y", y), series: .value("series", s.name))
                                .interpolationMethod(.linear)
                                .lineStyle(lineStyle(s.style))
                                .foregroundStyle(color(si))
                        }
                    }
                }
            }
            if let sel = selected, sel < model.columns.count, !model.isBar {
                let col = model.columns[sel]
                RuleMark(x: .value("x", col.x))
                    .lineStyle(StrokeStyle(lineWidth: 1))
                    .foregroundStyle(theme.text.opacity(0.28))
                ForEach(col.entries, id: \.series) { e in
                    if let p = model.series[e.series].points.first(where: { abs($0.x - col.x) < 1e-9 }), let y = p.y {
                        PointMark(x: .value("x", col.x), y: .value("y", y))
                            .symbolSize(lineWidth * lineWidth * 12)
                            .foregroundStyle(color(e.series))
                    }
                }
            }
        }
        .chartXScale(domain: model.x.min...model.x.max)
        .chartYScale(domain: model.y.min...model.y.max, type: model.y.scale == "log" ? .log : .linear)
        .chartXAxis {
            if model.x.scale == "category" || model.isBar {
                AxisMarks(values: Array(0..<(model.isBar ? model.categories.count : (model.x.categories?.count ?? 0))).map(Double.init)) { v in
                    AxisValueLabel {
                        if let i = v.as(Double.self).map({ Int($0.rounded()) }) {
                            Text(categoryLabel(i)).font(theme.font(font)).foregroundStyle(theme.placeholder)
                        }
                    }
                }
            } else {
                AxisMarks(values: model.x.ticks.map(\.value)) { v in
                    AxisValueLabel {
                        if let d = v.as(Double.self), let t = model.x.ticks.first(where: { abs($0.value - d) < 1e-6 }) {
                            Text(t.label).font(theme.font(font)).foregroundStyle(theme.placeholder)
                        }
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading, values: model.y.ticks.map(\.value)) { v in
                AxisGridLine().foregroundStyle(theme.separator)
                AxisValueLabel {
                    if let d = v.as(Double.self), let t = model.y.ticks.first(where: { abs($0.value - d) < 1e-6 }) {
                        Text(t.label).font(theme.font(font)).foregroundStyle(theme.placeholder)
                    }
                }
            }
        }
        .chartXAxisLabel(position: .bottom, alignment: .center) {
            if let l = model.x.label { Text(l).font(theme.font(font - 0.5)).foregroundStyle(theme.placeholder) }
        }
        .chartYAxisLabel(position: .topLeading) {
            if let l = model.y.label {
                Text(model.y.unit.isEmpty ? l : "\(l) (\(model.y.unit))").font(theme.font(font - 0.5)).foregroundStyle(theme.placeholder)
            }
        }
        .chartLegend(.hidden)
        .chartOverlay { proxy in
            GeometryReader { geo in
                Rectangle().fill(.clear).contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { g in
                                guard let frame = proxy.plotFrame else { return }
                                let xPos = g.location.x - geo[frame].origin.x
                                guard let x: Double = proxy.value(atX: xPos) else { return }
                                let near = model.columnNear(x)
                                if near != selected { Haptics.pick() }
                                selected = near
                            }
                    )
                    .onTapGesture { selected = nil }
                if let sel = selected, sel < model.columns.count, let frame = proxy.plotFrame {
                    let plot = geo[frame]
                    let px = proxy.position(forX: model.columns[sel].x) ?? 0
                    ChartReadout(model: model, column: sel, font: font)
                        .frame(maxWidth: 176)
                        .fixedSize()
                        .readoutPosition(x: plot.minX + px, plot: plot)
                }
            }
        }
    }

    private func categoryLabel(_ i: Int) -> String {
        if model.isBar { return i < model.categories.count ? model.categories[i] : "" }
        let cats = model.x.categories ?? []
        return i < cats.count ? cats[i] : ""
    }

    private func lineStyle(_ style: String) -> StrokeStyle {
        switch style {
        case "dashed": return StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round, dash: [lineWidth * 3, lineWidth * 2])
        case "dotted": return StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round, dash: [lineWidth * 0.5, lineWidth * 2])
        default: return StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round)
        }
    }

    @ViewBuilder
    private func annotationLabel(_ text: String?) -> some View {
        if let text, !text.isEmpty {
            Text(text).font(theme.font(font - 0.5)).foregroundStyle(theme.placeholder).padding(2)
        }
    }

    // ----- pie -----

    private var pie: some View {
        Chart {
            ForEach(Array((model.shares ?? []).enumerated()), id: \.offset) { i, share in
                SectorMark(angle: .value("share", share), angularInset: 1.2)
                    .foregroundStyle(ChartPalette.slice(model, i, dark: theme.isDark).opacity(selected == nil || selected == i ? 1 : 0.45))
                    .annotation(position: .overlay) {
                        if share >= 0.06 {
                            Text("\(Int((share * 100).rounded()))%")
                                .font(theme.font(font, weight: .semibold))
                                .foregroundStyle(theme.bubbleAssistant)
                        }
                    }
            }
        }
        .chartLegend(.hidden)
        .chartOverlay { proxy in
            GeometryReader { geo in
                Rectangle().fill(.clear).contentShape(Rectangle())
                    .gesture(DragGesture(minimumDistance: 0).onChanged { g in
                        guard let frame = proxy.plotFrame else { return }
                        let plot = geo[frame]
                        let c = CGPoint(x: plot.midX, y: plot.midY)
                        let dx = g.location.x - c.x, dy = g.location.y - c.y
                        var a = atan2(dx, -dy)
                        if a < 0 { a += 2 * .pi }
                        var acc = 0.0
                        var hit: Int? = nil
                        for (i, share) in (model.shares ?? []).enumerated() {
                            let end = acc + share * 2 * .pi
                            if Double(a) >= acc && Double(a) < end { hit = i; break }
                            acc = end
                        }
                        if hit != selected { Haptics.pick() }
                        selected = hit
                    })
                    .onTapGesture { selected = nil }
                if let sel = selected, sel < model.columns.count {
                    ChartReadout(model: model, column: sel, font: font)
                        .frame(maxWidth: 176)
                        .fixedSize()
                        .position(x: geo.size.width / 2, y: 24)
                }
            }
        }
    }
}

private extension View {
    /// Centre the readout over the crosshair at the plot's top, clamped inside the plot.
    func readoutPosition(x: CGFloat, plot: CGRect) -> some View {
        modifier(ReadoutPosition(x: x, plot: plot))
    }
}

private struct ReadoutPosition: ViewModifier {
    let x: CGFloat
    let plot: CGRect
    @State private var size: CGSize = .zero
    func body(content: Content) -> some View {
        content
            .background(GeometryReader { g in Color.clear.onAppear { size = g.size }.onChange(of: g.size) { _, s in size = s } })
            .position(x: min(max(x, plot.minX + size.width / 2), max(plot.maxX - size.width / 2, plot.minX + size.width / 2)),
                      y: plot.minY + size.height / 2 + 2)
    }
}

/// The readout chip: the x label, then dot · name · value per series.
struct ChartReadout: View {
    @Environment(\.theme) private var theme
    let model: ChartModel
    let column: Int
    var font: CGFloat = 12.5

    var body: some View {
        let col = model.columns[column]
        VStack(alignment: .leading, spacing: 4) {
            Text(col.label).font(theme.font(font, weight: .semibold)).foregroundStyle(theme.text)
            ForEach(col.entries, id: \.series) { e in
                HStack(spacing: 8) {
                    Circle()
                        .fill(model.isPie ? ChartPalette.slice(model, column, dark: theme.isDark) : ChartPalette.series(model, e.series, dark: theme.isDark))
                        .frame(width: 8, height: 8)
                    Text(model.isPie ? (column < model.categories.count ? model.categories[column] : "") : model.series[e.series].name)
                        .font(theme.font(font)).foregroundStyle(theme.muted)
                    Spacer(minLength: 12)
                    Text(e.text).font(theme.font(font)).foregroundStyle(theme.text).monospacedDigit()
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(theme.isDark ? Color(white: 0.17) : theme.card))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.25), radius: 7, y: 4)
        .accessibilityIdentifier("chart-readout")
    }
}

// MARK: - Save image

enum ChartImage {
    /// The picture for Photos: the card's title, legend and plot on the page colour, 3x.
    @MainActor
    static func render(model: ChartModel, theme: Theme, width: CGFloat = 390) -> UIImage? {
        let view = VStack(alignment: .leading, spacing: 8) {
            if let t = model.title { Text(t).font(theme.font(Theme.body, weight: .semibold)).foregroundStyle(theme.text) }
            if let s = model.subtitle { Text(s).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted) }
            if model.showLegend { ChartLegend(model: model, font: Theme.tiny, dot: 8) }
            ChartPlot(model: model, font: 12, lineWidth: 2.2).frame(height: 240)
            Text("Arbos").font(theme.font(11)).foregroundStyle(theme.placeholder)
        }
        .padding(20)
        .frame(width: width)
        .background(theme.bg)
        .environment(\.theme, theme)
        let renderer = ImageRenderer(content: view)
        renderer.scale = 3
        return renderer.uiImage
    }

    /// Save it to Photos (add-only permission, the media viewer's rule); false when refused
    /// or failed.
    @MainActor
    static func saveToPhotos(model: ChartModel, theme: Theme) async -> Bool {
        guard let image = render(model: model, theme: theme), let data = image.pngData() else { return false }
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else { return false }
        do {
            try await PHPhotoLibrary.shared().performChanges {
                let req = PHAssetCreationRequest.forAsset()
                req.addResource(with: .photo, data: data, options: nil)
            }
            return true
        } catch {
            return false
        }
    }
}

// MARK: - The expanded view (full-screen cover)

struct ChartViewer: View {
    @Environment(\.theme) private var theme
    let spec: String
    let model: ChartModel
    let onClose: () -> Void
    @State private var dataView = false
    @State private var copied = false
    @State private var toast: String? = nil
    @State private var shareImage: ShareImage? = nil

    var body: some View {
        ZStack(alignment: .top) {
            theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                HStack {
                    disc("xmark", id: "chart-viewer-close") { onClose() }
                    Spacer()
                    disc("square.and.arrow.up", id: "chart-viewer-share") {
                        shareImage = ChartImage.render(model: model, theme: theme).map(ShareImage.init)
                    }
                }
                .overlay {
                    Text(model.title ?? "Chart")
                        .font(theme.font(Theme.body, weight: .semibold))
                        .foregroundStyle(theme.text)
                        .lineLimit(1)
                        .padding(.horizontal, 60)
                        .accessibilityIdentifier("chart-viewer-title")
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                if let s = model.subtitle {
                    Text(s).font(theme.font(Theme.tiny)).foregroundStyle(theme.muted).padding(.top, 10)
                }
                if dataView {
                    ScrollView {
                        ChartDataTable(spec: spec).padding(16)
                    }
                    .accessibilityIdentifier("chart-viewer-data")
                } else {
                    VStack(alignment: .leading, spacing: 0) {
                        if model.showLegend {
                            ChartLegend(model: model, font: Theme.tiny, dot: 9)
                                .frame(maxWidth: .infinity, alignment: .center)
                                .padding(.top, 22)
                        }
                        ChartPlot(model: model, font: 12.5, lineWidth: 2.6)
                            .frame(height: 330)
                            .padding(.top, 8)
                        Text("Touch and hold to read values")
                            .font(theme.font(Theme.tiny))
                            .foregroundStyle(theme.placeholder)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 14)
                    }
                    .padding(.horizontal, 16)
                    .accessibilityIdentifier("chart-viewer-plot")
                }
                Picker("", selection: $dataView) {
                    Text("Chart").tag(false)
                    Text("Data").tag(true)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 40)
                .padding(.top, 22)
                .accessibilityIdentifier("chart-viewer-switch")
                Spacer(minLength: 0)
                HStack(spacing: 12) {
                    pill(copied ? "checkmark" : "doc.on.doc", copied ? "Copied" : "Copy data", id: "chart-viewer-copy", tint: copied ? theme.online : nil) {
                        guard let tsv = ChartModelCache.tsv(for: spec) else { return }
                        UIPasteboard.general.string = tsv
                        Haptics.pick()
                        copied = true
                        Task { @MainActor in
                            try? await Task.sleep(for: ChartCardMetrics.copiedFor)
                            copied = false
                        }
                    }
                    pill("square.and.arrow.down", "Save image", id: "chart-viewer-save", tint: nil) {
                        Task { @MainActor in
                            let ok = await ChartImage.saveToPhotos(model: model, theme: theme)
                            flash(ok ? "Saved to Photos" : "Couldn't save to Photos")
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            if let toast {
                Text(toast)
                    .font(theme.font(Theme.small, weight: .medium))
                    .foregroundStyle(theme.text)
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .background(Capsule().fill(theme.card))
                    .padding(.top, 70)
                    .transition(.opacity)
            }
        }
        .sheet(item: $shareImage) { share in
            ChartShareSheet(items: [share.image])
        }
    }

    private func flash(_ text: String) {
        withAnimation { toast = text }
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.8))
            if toast == text { withAnimation { toast = nil } }
        }
    }

    private func disc(_ symbol: String, id: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(theme.text)
                .frame(width: Controls.diameter, height: Controls.diameter)
                .background(Circle().fill(theme.card))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }

    private func pill(_ symbol: String, _ label: String, id: String, tint: Color?, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: symbol).font(.system(size: 16, weight: .medium)).foregroundStyle(tint ?? theme.muted)
                Text(label).font(theme.font(Theme.body, weight: .medium)).foregroundStyle(tint ?? theme.text)
            }
            .frame(maxWidth: .infinity)
            .frame(height: Controls.height)
            .background(Capsule().fill(theme.card))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }
}

/// The rendered picture for the share sheet (`sheet(item:)` needs an `Identifiable`).
struct ShareImage: Identifiable {
    let image: UIImage
    var id: ObjectIdentifier { ObjectIdentifier(image) }
}

/// The system share sheet for the rendered picture.
struct ChartShareSheet: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}

/// The Data view: the chart's table in the markdown table's style (header on the card fill,
/// muted cells, hairline grid), built from the core's TSV.
struct ChartDataTable: View {
    @Environment(\.theme) private var theme
    let spec: String

    /// Header cells, then one row per x / category.
    static func cells(tsv: String) -> (header: [String], rows: [[String]]) {
        let lines = tsv.split(separator: "\n", omittingEmptySubsequences: false).map { $0.components(separatedBy: "\t") }
        guard let header = lines.first else { return ([], []) }
        return (header, Array(lines.dropFirst()))
    }

    var body: some View {
        let (header, rows) = Self.cells(tsv: ChartModelCache.tsv(for: spec) ?? "")
        Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
            GridRow {
                ForEach(Array(header.enumerated()), id: \.offset) { i, h in
                    cell(h, head: true, first: i == 0)
                }
            }
            .background(theme.bg.opacity(0.6))
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                Divider().overlay(theme.separator).gridCellUnsizedAxes(.horizontal)
                GridRow {
                    ForEach(Array(row.enumerated()), id: \.offset) { i, c in
                        cell(c, head: false, first: i == 0)
                    }
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(theme.separator, lineWidth: 0.5))
        .accessibilityIdentifier("chart-viewer-table")
    }

    private func cell(_ text: String, head: Bool, first: Bool) -> some View {
        Text(text)
            .font(theme.font(Theme.small, weight: head ? .semibold : .regular))
            .foregroundStyle(head ? theme.text : theme.muted)
            .monospacedDigit()
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .leading) {
                if !first { Rectangle().fill(theme.separator).frame(width: 0.5) }
            }
    }
}
