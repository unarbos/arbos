import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// A photo or file picked into the composer, not sent yet. Photos are
/// re-encoded to JPEG under 2 MB so a message never carries more than the
/// kernel wants to hold; files go as they are, up to 20 MB.
struct PendingAttachment: Identifiable, Equatable {
    let id = UUID()
    let name: String
    let data: Data
    let isImage: Bool

    static let maxBytes = 20 * 1024 * 1024

    /// The name under `.arbos/attachments/`: unique, keeps the extension.
    var storedName: String {
        let ext = (name as NSString).pathExtension
        return ext.isEmpty ? id.uuidString : "\(id.uuidString).\(ext)"
    }

    var preview: UIImage? { isImage ? UIImage(data: data) : nil }

    static func == (lhs: PendingAttachment, rhs: PendingAttachment) -> Bool { lhs.id == rhs.id }

    /// A photo from the library, as JPEG.
    static func photo(_ item: PhotosPickerItem) async -> PendingAttachment? {
        guard let raw = try? await item.loadTransferable(type: Data.self), let image = UIImage(data: raw) else { return nil }
        var quality: CGFloat = 0.85
        var jpeg = image.jpegData(compressionQuality: quality)
        while let bytes = jpeg, bytes.count > 2 * 1024 * 1024, quality > 0.3 {
            quality -= 0.15
            jpeg = image.jpegData(compressionQuality: quality)
        }
        guard let bytes = jpeg else { return nil }
        return PendingAttachment(name: "photo-\(Int(Date().timeIntervalSince1970)).jpg", data: bytes, isImage: true)
    }

    /// A file from the Files picker.
    static func file(_ url: URL) -> PendingAttachment? {
        let opened = url.startAccessingSecurityScopedResource()
        defer { if opened { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url), data.count <= maxBytes else { return nil }
        let type = UTType(filenameExtension: url.pathExtension)
        return PendingAttachment(name: url.lastPathComponent, data: data, isImage: type?.conforms(to: .image) ?? false)
    }
}

/// The picked items above the field: a thumbnail for a photo, a name for
/// a file, an × on each. 56 pt tiles on the raised plate.
struct AttachmentChips: View {
    @Binding var attachments: [PendingAttachment]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(attachments) { file in
                    ZStack(alignment: .topTrailing) {
                        if let image = file.preview {
                            Image(uiImage: image)
                                .resizable()
                                .scaledToFill()
                                .frame(width: 56, height: 56)
                                .clipShape(RoundedRectangle(cornerRadius: ArbosTheme.cardRadius, style: .continuous))
                                // The remove button beside it already says
                                // "Remove <name>"; the chip itself said
                                // "Image".
                                .accessibilityLabel("Photo \(file.name)")
                        } else {
                            VStack(spacing: 4) {
                                Image(systemName: "doc")
                                    .font(.system(size: 18, weight: .regular))
                                    .foregroundStyle(ArbosTheme.textMuted)
                                Text(file.name)
                                    .font(.system(size: 9))
                                    .foregroundStyle(ArbosTheme.textFaint)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .padding(.horizontal, 4)
                            }
                            .frame(width: 72, height: 56)
                            .background(
                                RoundedRectangle(cornerRadius: ArbosTheme.cardRadius, style: .continuous)
                                    .fill(ArbosTheme.raised)
                            )
                        }
                        Button {
                            attachments.removeAll { $0.id == file.id }
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(Color.black)
                                .frame(width: 18, height: 18)
                                .background(Circle().fill(ArbosTheme.text))
                        }
                        .buttonStyle(.plain)
                        // Unlabelled it read as "Close" — the SF Symbol's own
                        // name, and the same word the call's end button used
                        // to answer to. Naming the file makes it clear which
                        // of several chips is being removed (M-314).
                        .accessibilityLabel("Remove \(file.name)")
                        .offset(x: 6, y: -6)
                    }
                }
            }
            .padding(.horizontal, ArbosTheme.barMargin + 8)
            .padding(.vertical, 6)
        }
    }
}
