import XCTest
import SwiftUI
import ArbosFFI
@testable import Arbos

/// Chart bubbles ([feedback chart-bubble-ios-2026-10-26]): the `chart` part parses with its
/// camelCase fields, becomes one block of the caption's bubble, hides its fallback picture, and
/// the core model behind Swift Charts (through the FFI) carries the domains, ticks, readout
/// columns and the TSV the Mac uses.
final class ChartCardTests: XCTestCase {
    private let chat = "fx-affine-main"
    private let t0: Int64 = 1_700_000_000_000

    static let spec: [String: Any] = [
        "kind": "line",
        "title": "Eval score over runs",
        "subtitle": "memory-pack eval",
        "series": [
            ["name": "Affine 2", "points": [[1, 66.2], [2, 70.4], [3, 79.6], [4, 83.4]]],
            ["name": "Baseline", "points": [[1, 70.1], [2, 70.6], [3, 71.1], [4, 71.2]]],
        ],
        "x": ["label": "Run"],
        "y": ["label": "Pass rate", "unit": "%", "min": 60, "max": 90],
        "annotations": [["type": "vline", "x": 3, "label": "prompt v3"]],
    ]

    private func chartMessage(fallbackPointsAt: String = "c") -> [String: Any] {
        [
            "id": "m2", "role": "assistant", "sender": "box:1", "status": "done", "createdAt": t0, "inReplyTo": "m1",
            "parts": [
                ["id": "t", "kind": "text", "text": "Affine 2 passed the baseline at run 2."],
                ["id": "a", "kind": "attachment", "attachment": ["name": "eval.png", "mime": "image/png", "ref": "blob-1", "bytes": 55_000, "kind": "image", "chartPart": fallbackPointsAt]],
                ["id": "c", "kind": "chart", "specVersion": 1, "chart": Self.spec, "table": "| Run | Affine 2 (%) | Baseline (%) |\n| --- | --- | --- |\n| 1 | 66.2 | 70.1 |", "fallback": "a"],
            ] as [[String: Any]],
        ]
    }

    func testThePartParsesWithItsCamelCaseFields() throws {
        let m = try XCTUnwrap(DocJSON.message(chartMessage(), chatId: chat))
        guard case .chart(let id, let version, let spec, let table, let fallback) = m.parts[2] else {
            return XCTFail("chart part: \(m.parts[2])")
        }
        XCTAssertEqual(id, "c")
        XCTAssertEqual(version, 1)
        XCTAssertTrue(spec.contains("\"kind\":\"line\""), spec)
        XCTAssertTrue(table.hasPrefix("| Run |"))
        XCTAssertEqual(fallback, "a")
        guard case .attachment(_, let a) = m.parts[1] else { return XCTFail("attachment part") }
        XCTAssertEqual(a.chartPart, "c")
        XCTAssertTrue(m.hasChartPart("c"))
        XCTAssertFalse(m.hasChartPart("zz"))
    }

    func testAChartPartJoinsTheCaptionsBubbleAndHidesItsPicture() throws {
        let m = try XCTUnwrap(DocJSON.message(chartMessage(), chatId: chat))
        let rows = Transcript.rows([m], projectId: "fx-affine")
        let bubbles = rows.compactMap { row -> [Block]? in
            if case .assistant(let blocks, _, _, _) = row.kind { return blocks }
            return nil
        }
        XCTAssertEqual(bubbles.count, 1, "caption and chart share one bubble: \(rows.map(\.id))")
        XCTAssertEqual(bubbles[0].count, 2)
        guard case .paragraph = bubbles[0][0] else { return XCTFail("the caption first") }
        guard case .chart(let partId, let spec, let table) = bubbles[0][1] else { return XCTFail("then the chart") }
        XCTAssertEqual(partId, "c")
        XCTAssertTrue(spec.contains("Eval score"))
        XCTAssertTrue(table.hasPrefix("| Run |"))
        XCTAssertFalse(rows.contains { if case .image = $0.kind { return true } else { return false } },
                       "the fallback picture is hidden: \(rows.map(\.id))")
        XCTAssertFalse(rows.contains { if case .file = $0.kind { return true } else { return false } })

        // The picture stands in for a chart that is not in this message: it stays a picture.
        let other = try XCTUnwrap(DocJSON.message(chartMessage(fallbackPointsAt: "elsewhere"), chatId: chat))
        let rows2 = Transcript.rows([other], projectId: "fx-affine")
        XCTAssertTrue(rows2.contains { if case .image = $0.kind { return true } else { return false } }
            || rows2.contains { if case .file = $0.kind { return true } else { return false } })
    }

    func testAChartWithoutACaptionIsItsOwnBubble() throws {
        var raw = chartMessage()
        raw["parts"] = Array((raw["parts"] as! [[String: Any]]).dropFirst())
        let m = try XCTUnwrap(DocJSON.message(raw, chatId: chat))
        let rows = Transcript.rows([m], projectId: "fx-affine")
        let bubbles = rows.compactMap { row -> [Block]? in
            if case .assistant(let blocks, _, _, _) = row.kind { return blocks }
            return nil
        }
        XCTAssertEqual(bubbles.count, 1)
        XCTAssertEqual(bubbles[0].count, 1)
        guard case .chart = bubbles[0][0] else { return XCTFail("the chart alone") }
    }

    func testTheCoreModelCarriesDomainsTicksColumnsAndTheTsv() throws {
        let data = try JSONSerialization.data(withJSONObject: Self.spec, options: [.sortedKeys])
        let spec = String(decoding: data, as: UTF8.self)
        XCTAssertNil(chartValidate(specJson: spec))
        let model = try XCTUnwrap(ChartModel.decode(try chartModelJson(specJson: spec)))
        XCTAssertEqual(model.kind, "line")
        XCTAssertEqual(model.series.count, 2)
        XCTAssertEqual(model.y.min, 60)
        XCTAssertEqual(model.y.max, 90)
        XCTAssertEqual(model.y.ticks.map { $0.label }, ["60%", "70%", "80%", "90%"])
        XCTAssertEqual(model.x.ticks.first?.label, "1")
        XCTAssertTrue(model.showLegend)
        XCTAssertEqual(model.columns.count, 4)
        XCTAssertEqual(model.columns[3].label, "Run 4")
        XCTAssertEqual(model.columns[3].entries[0].text, "83.4%")
        XCTAssertEqual(model.columnNear(3.8), 3)
        XCTAssertNil(model.columnNear(40))
        XCTAssertEqual(model.annotations.first?.kind, "vline")
        XCTAssertEqual(model.annotations.first?.x, 3)
        let tsv = try chartTableTsv(specJson: spec)
        XCTAssertEqual(tsv.split(separator: "\n").first.map(String.init), "Run\tAffine 2 (%)\tBaseline (%)")
        let cells = ChartDataTable.cells(tsv: tsv)
        XCTAssertEqual(cells.header, ["Run", "Affine 2 (%)", "Baseline (%)"])
        XCTAssertEqual(cells.rows.count, 4)
        XCTAssertEqual(cells.rows[3], ["4", "83.4", "71.2"])
        XCTAssertTrue(chartValidate(specJson: "{\"kind\":\"pie\"}")?.contains("series") ?? false)
    }

    func testThePaletteFollowsTheThemeAndAGivenColourIsKept() throws {
        let data = try JSONSerialization.data(withJSONObject: Self.spec, options: [.sortedKeys])
        let model = try XCTUnwrap(ChartModel.decode(try chartModelJson(specJson: String(decoding: data, as: UTF8.self))))
        XCTAssertNotEqual(ChartPalette.series(model, 0, dark: true), ChartPalette.series(model, 0, dark: false))
        XCTAssertEqual(ChartPalette.series(model, 0, dark: true), ChartPalette.darkColors[0])
        XCTAssertEqual(ChartPalette.hex("#1084FE"), Color(red: 0x10 / 255, green: 0x84 / 255, blue: 0xFE / 255))
        XCTAssertNil(ChartPalette.hex("red"))
        var own = model
        own.series[0].color = "#FF0000"
        XCTAssertEqual(ChartPalette.series(own, 0, dark: true), ChartPalette.series(own, 0, dark: false))
        XCTAssertEqual(ChartCardMetrics.height(kind: "bar"), ChartCardMetrics.barPlotHeight)
        XCTAssertEqual(ChartCardMetrics.iconSquare, 24)
    }

    /// Source check: the transcript's block switch draws a `ChartCard` for `.chart`, with the
    /// code card's three-square chrome and the full-screen cover behind Expand.
    func testTheCardSourceCarriesTheChrome() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        let transcript = try String(contentsOf: root.appendingPathComponent("Arbos/UI/Transcript.swift"), encoding: .utf8)
        XCTAssertTrue(transcript.contains("case .chart(let partId, let spec, _):\n            ChartCard(partId: partId, spec: spec)"))
        let card = try String(contentsOf: root.appendingPathComponent("Arbos/UI/ChartCard.swift"), encoding: .utf8)
        for needle in ["ChartIconSquares", "\"chart-copy\"", "\"chart-save\"", "\"chart-expand\"", ".fullScreenCover(isPresented: $expanded)",
                       "ChartScrubHost(", "ImageRenderer(content: view)", "PHPhotoLibrary.requestAuthorization(for: .addOnly)",
                       "Picker(\"\", selection: $dataView)", "chartModelJson(specJson:", "chartTableTsv(specJson:"] {
            XCTAssertTrue(card.contains(needle), needle)
        }
        // #2122 rule: nothing competing with the transcript scroll is a SwiftUI drag.
        XCTAssertFalse(card.contains("DragGesture("), "the plot's touch surface is UIKit recognizers that can fail")
    }

    /// The plot's pan claims a mostly horizontal finger and declines at once otherwise, so a
    /// vertical pan that starts on a chart scrolls the transcript (iphone-scroll-perf #2122).
    func testTheScrubClaimsHorizontalPansAndYieldsVerticalOnesToTheScroll() {
        XCTAssertEqual(ChartScrub.claim(CGSize(width: 12, height: 0)), .claimed)
        XCTAssertEqual(ChartScrub.claim(CGSize(width: -12, height: 4)), .claimed)
        XCTAssertEqual(ChartScrub.claim(CGSize(width: 0, height: 12)), .declined)
        XCTAssertEqual(ChartScrub.claim(CGSize(width: 8, height: -8)), .declined, "a diagonal is the scroll's")
        XCTAssertEqual(ChartScrub.claim(CGSize(width: 10, height: 9)), .declined, "needs a clear horizontal lead")
        XCTAssertLessThanOrEqual(ChartScrub.minimumDistance, BackSwipe.lockDistance, "decides no later than the full-screen back swipe")
        XCTAssertLessThan(ChartScrub.holdDuration, 0.35, "the chart hold begins before the row's long press")
    }
}
