// Copy of a message, the pasteboard side ([feedback copy-image-messages-ios-2026-10-27]).
//
// Before this, the long-press menu's Copy put `plainText` on the pasteboard and nothing
// else: a picture message copied nothing, a picture with a caption copied the caption.
// Now every Copy — the bubble menu, the media viewer's pill, the Library row — goes
// through `MessagePasteboard.items` + `UIPasteboard.setItems`:
//
// - every picture of the message is one **pasteboard item** carrying its **original
//   bytes under its own type** (PNG stays `public.png`, JPEG `public.jpeg`, GIF
//   `com.compuserve.gif`, WebP `org.webmproject.webp`, HEIC `public.heic`) plus a
//   `public.png` re-encode when the native type is not PNG — the representation
//   `UIPasteboard.image` / `UIImage(pasteboard:)` readers take, so a GIF pastes as a
//   GIF into Messages and as a still into a plain image well;
// - the message's text rides on the **first** item as `public.utf8-plain-text`, so a
//   text field receives the words and an image well the picture — one rich item;
// - several pictures become several items in message order;
// - a file that is not a picture travels as its own data under its file type
//   (`com.adobe.pdf`, …) so Files / Mail / the composer can take it.
//
// The twin of the Mac's `crates/arbos-mac/src/pasteboard.rs`; the items it lays out
// are plain `[UTI: value]` dictionaries so the layout is testable without a device.

import Foundation
import ImageIO
import UIKit
import UniformTypeIdentifiers

enum MessagePasteboard {
    /// The bytes of one attachment as fetched from the relay.
    struct Fetched: Equatable {
        var name: String
        /// The relay's `content-type`; the attachment's own `mime` when the relay's is generic.
        var mime: String
        var data: Data
    }

    static let png = "public.png"
    static let jpeg = "public.jpeg"
    static let gif = "com.compuserve.gif"
    static let webp = "org.webmproject.webp"
    static let heic = "public.heic"
    static let tiff = "public.tiff"
    static let bmp = "com.microsoft.bmp"
    static let text = "public.utf8-plain-text"

    /// The image types we declare natively, most to least common.
    static let imageTypes = [png, jpeg, gif, webp, heic, tiff, bmp]
    /// Reading an item back, the native picture before any twin: a GIF / WebP / HEIC item
    /// also carries a `public.png` re-encode (ours and other apps' alike), a JPEG item a
    /// PNG twin, a screenshot PNG a TIFF — so animated and lossy originals come first, PNG
    /// before TIFF.
    static let readPreference = [gif, webp, heic, jpeg, png, tiff, bmp]

    /// The native type of a fetched picture: the bytes decide (the relay's `content-type`
    /// and the attachment's `mime` have been wrong before — a `.png` that is a JPEG),
    /// then the MIME, then the file name. `nil` for anything that is not a still picture.
    static func imageType(of f: Fetched) -> String? {
        if let sniffed = sniff(f.data) { return sniffed }
        let mime = f.mime.lowercased().split(separator: ";").first.map(String.init)?
            .trimmingCharacters(in: .whitespaces) ?? ""
        switch mime {
        case "image/png": return png
        case "image/jpeg", "image/jpg": return jpeg
        case "image/gif": return gif
        case "image/webp": return webp
        case "image/heic", "image/heif": return heic
        case "image/tiff": return tiff
        case "image/bmp", "image/x-ms-bmp": return bmp
        default: break
        }
        switch (f.name as NSString).pathExtension.lowercased() {
        case "png": return png
        case "jpg", "jpeg": return jpeg
        case "gif": return gif
        case "webp": return webp
        case "heic", "heif": return heic
        case "tif", "tiff": return tiff
        case "bmp": return bmp
        default: return nil
        }
    }

    private static func sniff(_ d: Data) -> String? {
        guard d.count >= 12 else { return nil }
        let b = [UInt8](d.prefix(12))
        if b[0] == 0x89, b[1] == 0x50, b[2] == 0x4E, b[3] == 0x47 { return png }
        if b[0] == 0xFF, b[1] == 0xD8, b[2] == 0xFF { return jpeg }
        if b[0] == 0x47, b[1] == 0x49, b[2] == 0x46, b[3] == 0x38 { return gif }
        if b[0] == 0x52, b[1] == 0x49, b[2] == 0x46, b[3] == 0x46, b[8] == 0x57, b[9] == 0x45, b[10] == 0x42, b[11] == 0x50 { return webp }
        // ISO BMFF `ftyp` box with a HEIF brand (`heic`, `heix`, `mif1`, …).
        if b[4] == 0x66, b[5] == 0x74, b[6] == 0x79, b[7] == 0x70,
           (b[8] == 0x68 && b[9] == 0x65 && b[10] == 0x69) || (b[8] == 0x6D && b[9] == 0x69 && b[10] == 0x66) { return heic }
        if (b[0] == 0x49 && b[1] == 0x49 && b[2] == 0x2A && b[3] == 0x00) || (b[0] == 0x4D && b[1] == 0x4D && b[2] == 0x00 && b[3] == 0x2A) { return tiff }
        if b[0] == 0x42, b[1] == 0x4D { return bmp }
        return nil
    }

    /// The `public.png` twin of a picture that is not a PNG (the first frame of a GIF);
    /// `nil` for a PNG and for bytes UIKit cannot decode.
    static func pngTwin(_ data: Data, type: String) -> Data? {
        guard type != png, let image = UIImage(data: data) else { return nil }
        return image.pngData()
    }

    /// The type a non-picture file is declared under: from its MIME, else its extension,
    /// else plain data.
    static func fileType(name: String, mime: String) -> String {
        if let t = UTType(mimeType: mime), t != .data { return t.identifier }
        let ext = (name as NSString).pathExtension
        if !ext.isEmpty, let t = UTType(filenameExtension: ext) { return t.identifier }
        return UTType.data.identifier
    }

    /// The pasteboard items for a message: `text` and its attachments' bytes in message
    /// order (`fetched` is what came back; a blob that did not is simply not there).
    /// Each item is `[UTI: Data | String]` — what `UIPasteboard.setItems` takes.
    static func items(text: String, fetched: [Fetched]) -> [[String: Any]] {
        var out: [[String: Any]] = []
        for f in fetched where !f.data.isEmpty {
            var item: [String: Any] = [:]
            if let type = imageType(of: f) {
                item[type] = f.data
                if let twin = pngTwin(f.data, type: type) { item[png] = twin }
            } else {
                item[fileType(name: f.name, mime: f.mime)] = f.data
            }
            out.append(item)
        }
        let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !words.isEmpty {
            if out.isEmpty {
                out.append([Self.text: words])
            } else {
                out[0][Self.text] = words
            }
        }
        return out
    }

    /// Put `items` on the general pasteboard, replacing what was there.
    static func write(_ items: [[String: Any]]) {
        UIPasteboard.general.setItems(items, options: [:])
    }

    // MARK: - Reading back (the composer's paste)

    /// Does the general pasteboard hold a picture or a file the composer can stage?
    static func hasMedia(_ pb: UIPasteboard = .general) -> Bool {
        !pendingAttachments(from: pb).isEmpty
    }

    /// Every pasteboard item that is a picture or a file, as composer attachments in
    /// pasteboard order: the first declared image type we know (the native one — a GIF
    /// comes back as a GIF, not its PNG twin), else the item's first file-like type.
    /// Items that are only text are skipped.
    static func pendingAttachments(from pb: UIPasteboard = .general) -> [AppModel.PendingAttachment] {
        var out: [AppModel.PendingAttachment] = []
        let stamp = Self.stamp.string(from: Date())
        let perItem: [[String]] = pb.types(forItemSet: nil) ?? []
        for (ix, itemTypes) in perItem.enumerated() {
            if let native = readPreference.first(where: { itemTypes.contains($0) }),
               let data = pb.data(forPasteboardType: native, inItemSet: IndexSet(integer: ix))?.first, !data.isEmpty {
                let ext = UTType(native)?.preferredFilenameExtension ?? "png"
                let mime = UTType(native)?.preferredMIMEType ?? "image/\(ext)"
                let n = out.count + 1
                out.append(.init(name: "Pasted image \(stamp)\(n > 1 ? " (\(n))" : "").\(ext)", mime: mime, data: data))
                continue
            }
            if let fileType = itemTypes.compactMap({ UTType($0) }).first(where: { t in
                t.conforms(to: .data) && !t.conforms(to: .text) && !t.conforms(to: .url) && t != .data
            }), let data = pb.data(forPasteboardType: fileType.identifier, inItemSet: IndexSet(integer: ix))?.first, !data.isEmpty {
                let ext = fileType.preferredFilenameExtension ?? "bin"
                out.append(.init(name: "Pasted file \(stamp).\(ext)", mime: fileType.preferredMIMEType ?? "application/octet-stream", data: data))
            }
        }
        return out
    }

    #if DEBUG
    /// Rig: every item of the general pasteboard — declared types, byte sizes, decoded pixel size.
    static func debugDump(_ pb: UIPasteboard = .general) {
        let perItem: [[String]] = pb.types(forItemSet: nil) ?? []
        print("pasteboard: \(perItem.count) item(s), changeCount \(pb.changeCount)")
        for (ix, types) in perItem.enumerated() {
            print(" item \(ix): types \(types)")
            for t in types {
                guard let data = pb.data(forPasteboardType: t, inItemSet: IndexSet(integer: ix))?.first else { continue }
                if t == text, let s = String(data: data, encoding: .utf8) {
                    print("  \(t): \"\(s)\"")
                } else if let image = UIImage(data: data) {
                    let frames = (CGImageSourceCreateWithData(data as CFData, nil)).map { CGImageSourceGetCount($0) } ?? 1
                    print("  \(t): \(data.count) bytes, \(Int(image.size.width * image.scale))x\(Int(image.size.height * image.scale)) px, \(frames) frame(s)")
                } else {
                    print("  \(t): \(data.count) bytes")
                }
            }
        }
        print(" UIPasteboard.string = \(pb.string.map { "\"\($0)\"" } ?? "nil"), hasImages = \(pb.hasImages), image = \(pb.image.map { "\(Int($0.size.width * $0.scale))x\(Int($0.size.height * $0.scale))" } ?? "nil")")
    }
    #endif

    private static let stamp: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return f
    }()
}

// MARK: - The copy actions

extension AppModel {
    /// Copy of a whole message — the long-press menu's Copy: its body (`Message.body`,
    /// the accessor Forward reads too) as `MessagePasteboard` lays it out. Words alone
    /// land at once; pictures and files are fetched from the relay first (the original
    /// bytes, never a thumbnail) and the pasteboard is written when they are in.
    /// `slot` is where the chat's blobs live (`<project id>` or `people:<chat id>`).
    @MainActor
    func copyMessage(_ message: Message, slot: String) async {
        guard let body = message.body else {
            notice = Notice(text: message.hasUploadingAttachment ? "Still uploading — try again in a moment" : "Nothing to copy")
            return
        }
        await copy(text: body.text, attachments: body.attachments, slot: slot)
    }

    /// Copy of one attachment on its own — the media viewer's pill, the Library row:
    /// the picture (or file) without the message's words. `true` when it was copied.
    @MainActor
    @discardableResult
    func copyAttachment(_ attachment: Attachment, slot: String) async -> Bool {
        guard attachment.isOnRelay else {
            notice = Notice(text: attachment.blobRef?.hasPrefix("local:") == true ? "Still uploading — try again in a moment" : "Nothing to copy")
            return false
        }
        return await copy(text: "", attachments: [attachment], slot: slot)
    }

    @MainActor
    @discardableResult
    private func copy(text: String, attachments: [Attachment], slot: String) async -> Bool {
        if attachments.isEmpty {
            MessagePasteboard.write(MessagePasteboard.items(text: text, fetched: []))
            return true
        }
        var fetched: [MessagePasteboard.Fetched] = []
        for a in attachments {
            let kind = MediaAlbum.kind(of: a) ?? .file
            let item = MediaItem(id: "copy#\(a.blobRef ?? a.name)", name: a.name, mime: a.mime, kind: kind,
                                 blobRef: a.blobRef, slot: slot, by: "", at: 0)
            guard let (data, mime) = await MediaFetch.bytes(for: item, model: self) else { continue }
            let generic = mime.isEmpty || mime.hasPrefix("application/octet-stream")
            fetched.append(.init(name: a.name, mime: generic ? a.mime : mime, data: data))
        }
        let items = MessagePasteboard.items(text: text, fetched: fetched)
        guard !items.isEmpty else {
            notice = Notice(text: "Couldn't copy — the picture didn't download. Try again.")
            return false
        }
        MessagePasteboard.write(items)
        let missing = attachments.count - fetched.count
        if missing > 0 {
            notice = Notice(text: missing == 1 ? "Copied — one picture didn't download" : "Copied — \(missing) pictures didn't download")
        } else {
            switch (fetched.count, text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) {
            case (1, true): notice = Notice(text: "Copied the picture")
            case (1, false): notice = Notice(text: "Copied the picture and its text")
            case (let n, true): notice = Notice(text: "Copied \(n) pictures")
            case (let n, false): notice = Notice(text: "Copied \(n) pictures and the text")
            }
        }
        return true
    }
}
