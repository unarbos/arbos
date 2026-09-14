// Draws the release artwork from the one source logo (bundle/icon.png, the
// trunk mark over a wordmark, black on white):
//
//   swift bundle/artwork.swift icon  <logo.png> <out-1024.png>
//   swift bundle/artwork.swift dmg   <logo.png> <out-background@2x.png>
//
// `icon`: a macOS app icon — the HIG squircle on the 1024 canvas (artwork
// 824 px wide, centred), a soft white gradient with a hairline edge, the
// trunk mark alone (the wordmark is cut) at 52 % of the tile. Rendered from
// the vector-like mark at full resolution so every .icns size is crisp.
//
// `dmg`: the disk-image window backdrop at 2x (1320 × 800 for a 660 × 400
// window): the mark faint on the left, an arrow, "Applications" on the
// right, one line of instruction. The Makefile places the two icons over
// the two spots.
//
// Only Foundation, CoreGraphics, CoreText and ImageIO: it runs on a bare macOS.

import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(1)
}

func load(_ path: String) -> CGImage {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil)
    else { fail("cannot read \(path)") }
    return img
}

func save(_ image: CGImage, _ path: String) {
    guard let dest = CGImageDestinationCreateWithURL(
        URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { fail("cannot write \(path)") }
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) { fail("cannot finalize \(path)") }
}

/// Grey-level pixels of the logo, top row first: 0 = black ink, 255 = paper.
func luma(_ image: CGImage) -> (w: Int, h: Int, px: [UInt8]) {
    let w = image.width, h = image.height
    var px = [UInt8](repeating: 255, count: w * h)
    let cs = CGColorSpaceCreateDeviceGray()
    guard let ctx = CGContext(data: &px, width: w, height: h, bitsPerComponent: 8,
                              bytesPerRow: w, space: cs, bitmapInfo: CGImageAlphaInfo.none.rawValue)
    else { fail("gray context") }
    ctx.setFillColor(gray: 1, alpha: 1)
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    return (w, h, px)
}

/// The trunk mark alone, as black ink over transparency. Ink rows of the
/// logo are grouped into bands separated by blank rows; the tallest band is
/// the mark (the wordmark under it is the short one). Paper becomes alpha,
/// so the mark can sit on any tile.
func markImage(_ image: CGImage) -> CGImage {
    let (w, h, px) = luma(image)
    func inkRow(_ y: Int) -> (Int, Int)? {
        var lo = w, hi = -1
        for x in 0..<w where px[y * w + x] < 128 { lo = min(lo, x); hi = max(hi, x) }
        return hi >= 0 ? (lo, hi) : nil
    }
    var bands: [(y0: Int, y1: Int, x0: Int, x1: Int)] = []
    var cur: (Int, Int, Int, Int)? = nil
    for y in 0..<h {
        if let (lo, hi) = inkRow(y) {
            if var c = cur { c.1 = y; c.2 = min(c.2, lo); c.3 = max(c.3, hi); cur = c }
            else { cur = (y, y, lo, hi) }
        } else if let c = cur { bands.append((c.0, c.1, c.2, c.3)); cur = nil }
    }
    if let c = cur { bands.append((c.0, c.1, c.2, c.3)) }
    guard let m = bands.max(by: { ($0.y1 - $0.y0) < ($1.y1 - $1.y0) }) else { fail("no ink in logo") }
    let mw = m.x1 - m.x0 + 1, mh = m.y1 - m.y0 + 1
    // Premultiplied RGBA, black ink: every channel is 0, alpha is the ink.
    var out = [UInt8](repeating: 0, count: mw * mh * 4)
    for y in 0..<mh {
        for x in 0..<mw {
            let ink = 255 - Int(px[(m.y0 + y) * w + (m.x0 + x)])
            out[(y * mw + x) * 4 + 3] = UInt8(ink)
        }
    }
    let data = Data(out) as CFData
    guard let provider = CGDataProvider(data: data),
          let mark = CGImage(width: mw, height: mh, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: mw * 4,
                             space: CGColorSpace(name: CGColorSpace.sRGB)!,
                             bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                             provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    else { fail("mark image") }
    return mark
}

func squircle(in rect: CGRect) -> CGPath {
    // Apple's icon corner: about 22.37 % of the side, continuous curvature.
    // A plain rounded rect at that radius is within a pixel of it at 1024.
    return CGPath(roundedRect: rect, cornerWidth: rect.width * 0.2237, cornerHeight: rect.height * 0.2237, transform: nil)
}

func rgbaContext(_ w: Int, _ h: Int) -> CGContext {
    guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                              space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { fail("rgba context") }
    ctx.interpolationQuality = .high
    return ctx
}

func drawIcon(logo: CGImage, out: String) {
    let size = 1024
    let ctx = rgbaContext(size, size)
    // HIG: artwork occupies 824 of 1024, centred, with the rest transparent
    // so the Dock's shadow and spacing match every other app.
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let path = squircle(in: tile)

    // Shadow under the tile, then the tile itself.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -6), blur: 18, color: CGColor(gray: 0, alpha: 0.28))
    ctx.addPath(path)
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()

    // Paper: white at the top to a faint warm grey at the foot.
    ctx.saveGState()
    ctx.addPath(path)
    ctx.clip()
    let colors = [CGColor(red: 1, green: 1, blue: 1, alpha: 1),
                  CGColor(red: 0.93, green: 0.93, blue: 0.92, alpha: 1)] as CFArray
    let grad = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!, colors: colors, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.minY), options: [])
    ctx.restoreGState()

    // Hairline edge so the tile reads on a white desktop.
    ctx.saveGState()
    ctx.addPath(squircle(in: tile.insetBy(dx: 1.5, dy: 1.5)))
    ctx.setStrokeColor(CGColor(gray: 0, alpha: 0.10))
    ctx.setLineWidth(3)
    ctx.strokePath()
    ctx.restoreGState()

    // The mark: 52 % of the tile's height, centred, a touch above middle.
    let mark = markImage(logo)
    let targetH = tile.height * 0.52
    let scale = targetH / CGFloat(mark.height)
    let targetW = CGFloat(mark.width) * scale
    let origin = CGPoint(x: tile.midX - targetW / 2, y: tile.midY - targetH / 2 + tile.height * 0.02)
    ctx.draw(mark, in: CGRect(origin: origin, size: CGSize(width: targetW, height: targetH)))

    guard let image = ctx.makeImage() else { fail("icon image") }
    save(image, out)
}

func drawDmgBackground(logo: CGImage, out: String) {
    // 660 × 400 points at 2x. The Makefile's Finder layout puts the app icon
    // at (165, 190) and the Applications link at (495, 190) in points.
    let w = 1320, h = 800
    let ctx = rgbaContext(w, h)
    ctx.setFillColor(CGColor(red: 0.97, green: 0.97, blue: 0.96, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))

    // Arrow between the two spots (points 165 → 495 at y 190, in 2x pixels;
    // CoreGraphics y is bottom-up so 190 pt from the top is 800 - 380).
    let y = CGFloat(h) - 380
    let x0: CGFloat = 470, x1: CGFloat = 850
    ctx.setStrokeColor(CGColor(gray: 0.55, alpha: 1))
    ctx.setLineWidth(10)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.move(to: CGPoint(x: x0, y: y)); ctx.addLine(to: CGPoint(x: x1, y: y))
    ctx.move(to: CGPoint(x: x1 - 60, y: y + 48)); ctx.addLine(to: CGPoint(x: x1, y: y)); ctx.addLine(to: CGPoint(x: x1 - 60, y: y - 48))
    ctx.strokePath()

    // A faint mark bottom-left as the brand, and the instruction line.
    let mark = markImage(logo)
    let mh: CGFloat = 120
    let mw = CGFloat(mark.width) * mh / CGFloat(mark.height)
    ctx.saveGState()
    ctx.setAlpha(0.08)
    ctx.draw(mark, in: CGRect(x: 60, y: 60, width: mw, height: mh))
    ctx.restoreGState()

    let text = "Drag Arbos to Applications, then open it from there." as CFString
    let font = CTFontCreateWithName("Helvetica Neue" as CFString, 34, nil)
    let attrs: [CFString: Any] = [kCTFontAttributeName: font,
                                  kCTForegroundColorAttributeName: CGColor(gray: 0.35, alpha: 1)]
    let line = CTLineCreateWithAttributedString(CFAttributedStringCreate(nil, text, attrs as CFDictionary))
    let bounds = CTLineGetBoundsWithOptions(line, [])
    ctx.textPosition = CGPoint(x: (CGFloat(w) - bounds.width) / 2, y: 150)
    CTLineDraw(line, ctx)

    guard let image = ctx.makeImage() else { fail("dmg image") }
    save(image, out)
}

let args = CommandLine.arguments
guard args.count == 4 else { fail("usage: artwork.swift icon|dmg <logo.png> <out.png>") }
let logo = load(args[2])
switch args[1] {
case "icon": drawIcon(logo: logo, out: args[3])
case "dmg": drawDmgBackground(logo: logo, out: args[3])
default: fail("unknown mode \(args[1])")
}
