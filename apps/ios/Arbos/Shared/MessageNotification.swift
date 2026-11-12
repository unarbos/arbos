// A message notification, Grok Bot style: the sender's picture (an agent's
// tree in its colour from the bundled v2 assets, or a person's GitHub
// avatar) on the banner and lock screen, the preview as the body. Shared by
// the app (local notifications from the /me/pushes poll) and the
// Notification Service Extension (remote pushes).
//
// Payload (relay → APNs, `mutable-content: 1`):
//   aps: { alert: {title, body}, badge, sound, "thread-id": <chat_id>, category: "ARBOS_MESSAGE" }
//   chat_id, project_id ("people" for DMs), message_id, chat_title (groups),
//   sender: { kind: "agent"|"user", id, name, handle, avatar, avatar_color, avatar_url }
// Flat `sender_*` keys are accepted too.

import UserNotifications
import Intents
import UIKit
import os

let messageCategory = "ARBOS_MESSAGE"
private let log = Logger(subsystem: "com.unarbos.arbos.ios", category: "message-notification")

struct MessagePush {
    let chatId: String
    let projectId: String
    let title: String
    private(set) var body: String
    let senderKind: String
    let senderId: String
    let senderName: String
    let senderHandle: String
    let avatarSlug: String?
    let avatarColor: String?
    let avatarUrl: String?
    /// D79 `sender.avatar_color_index_effective` when the relay sends it.
    let avatarColorIndex: Int?
    /// The chat's name for a group / shared project chat (`chat_title`); nil for a DM. The
    /// banner is titled with it and the sender goes in the body ("Sender: text") — the
    /// communication intent then shows group name / sender / text (2026-10-27).
    let groupName: String?
    /// `dm` | `group` | `project` | `channel` (`chat_kind`; an older relay sends none).
    let chatKind: String
    /// The message alone (`text`), for the intent's content when the body carries "Sender: ".
    private(set) var text: String?
    /// The encrypted event this alert announces and its room (`matrix`), when the relay could
    /// not read the message and said only that there is one: the notification extension
    /// reads it on the device (phase 5).
    let sealed: (roomId: String, event: String)?
    /// Tokenised URL of the group picture (app-only: the extension has no relay token and
    /// draws the group's initials instead).
    let groupImageUrl: String?

    init?(userInfo: [AnyHashable: Any]) {
        let aps = userInfo["aps"] as? [String: Any] ?? [:]
        let category = (aps["category"] as? String) ?? (userInfo["category"] as? String) ?? ""
        guard category == messageCategory, let chat = userInfo["chat_id"] as? String, !chat.isEmpty else { return nil }
        let alert = aps["alert"] as? [String: Any] ?? [:]
        let sender = userInfo["sender"] as? [String: Any] ?? [:]
        func s(_ nested: String, _ flat: String) -> String? {
            let v = (sender[nested] as? String) ?? (userInfo[flat] as? String)
            return (v?.isEmpty == false) ? v : nil
        }
        chatId = chat
        projectId = (userInfo["project_id"] as? String) ?? "people"
        title = (alert["title"] as? String) ?? ""
        body = (alert["body"] as? String) ?? (aps["alert"] as? String) ?? ""
        senderKind = s("kind", "sender_kind") ?? (s("avatar", "sender_avatar") != nil ? "agent" : "user")
        senderId = s("id", "sender_id") ?? s("handle", "sender_handle") ?? chat
        senderHandle = s("handle", "sender_handle") ?? ""
        senderName = s("name", "sender_name") ?? (senderHandle.isEmpty ? title : senderHandle)
        avatarSlug = s("avatar", "sender_avatar")
        avatarColor = s("avatar_color", "sender_avatar_color")
        avatarUrl = s("avatar_url", "sender_avatar_url")
        avatarColorIndex = ((sender["avatar_color_index_effective"] as? NSNumber) ?? (sender["avatar_color_index"] as? NSNumber)
                            ?? (userInfo["sender_avatar_color_index"] as? NSNumber))?.intValue
        groupName = (userInfo["chat_title"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        chatKind = (userInfo["chat_kind"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (groupName == nil ? "dm" : "group")
        text = (userInfo["text"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        groupImageUrl = (userInfo["chat_image_url"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        let matrix = userInfo["matrix"] as? [String: Any] ?? [:]
        if let room = matrix["room_id"] as? String, room.hasPrefix("!"), room.utf8.count <= 255,
           let event = matrix["event"] as? [String: Any],
           let json = try? JSONSerialization.data(withJSONObject: event) {
            sealed = (room, String(decoding: json, as: UTF8.self))
        } else {
            sealed = nil
        }
    }

    /// Say `words` where the alert said only that there is a message.
    mutating func show(_ words: String) {
        if let placeholder = text, body.hasSuffix(placeholder) {
            body = body.dropLast(placeholder.count) + words
        } else {
            body = words
        }
        text = words
    }

    /// The same shape built locally (the /me/pushes poll, the debug hook).
    static func userInfo(chatId: String, projectId: String, title: String, body: String, senderKind: String, senderId: String,
                         senderName: String, senderHandle: String, avatar: String?, avatarColor: String?, avatarUrl: String?,
                         chatTitle: String? = nil, chatKind: String? = nil, text: String? = nil,
                         chatImageUrl: String? = nil) -> [AnyHashable: Any] {
        var sender: [String: Any] = ["kind": senderKind, "id": senderId, "name": senderName, "handle": senderHandle]
        if let avatar { sender["avatar"] = avatar }
        if let avatarColor { sender["avatar_color"] = avatarColor }
        if let avatarUrl { sender["avatar_url"] = avatarUrl }
        var info: [AnyHashable: Any] = [
            "aps": ["alert": ["title": title, "body": body], "category": messageCategory, "thread-id": chatId] as [String: Any],
            "chat_id": chatId, "project_id": projectId, "sender": sender,
        ]
        if let chatTitle { info["chat_title"] = chatTitle }
        if let chatKind { info["chat_kind"] = chatKind }
        if let text { info["text"] = text }
        if let chatImageUrl { info["chat_image_url"] = chatImageUrl }
        return info
    }

    /// The group's picture for the intent's conversation image: the real one when the app
    /// has a tokenised URL, else the group's initials in the palette colour of its id.
    @MainActor
    func groupImage() async -> INImage? {
        guard let groupName else { return nil }
        if let urlString = groupImageUrl, let url = URL(string: urlString) {
            var req = URLRequest(url: url)
            req.timeoutInterval = 6
            if let (data, _) = try? await URLSession.shared.data(for: req), !data.isEmpty,
               let round = AvatarImage.roundPNG(data, pixels: 192) {
                return INImage(imageData: round)
            }
        }
        if let png = AvatarPalette.initialsPNG(name: groupName, handle: nil, id: chatId, pixels: 192, dark: Self.systemIsDark) {
            return INImage(imageData: png)
        }
        return nil
    }

    /// The person's picture when there is one, else the initials circle (D79).
    @MainActor
    func senderImage() async -> INImage? {
        if let urlString = avatarUrl, let url = URL(string: urlString) {
            var req = URLRequest(url: url)
            req.timeoutInterval = 6
            if let (data, _) = try? await URLSession.shared.data(for: req), !data.isEmpty,
               let round = AvatarImage.roundPNG(data, pixels: 192) {
                return INImage(imageData: round)
            }
        }
        if let png = AvatarPalette.initialsPNG(name: senderName, handle: senderHandle, id: senderId, override: avatarColorIndex,
                                               pixels: 192, dark: Self.systemIsDark) {
            return INImage(imageData: png)
        }
        return nil
    }

    /// The device's light/dark setting (D81): the system draws the banner, so the
    /// circle takes the dark stops when the device is dark, whatever the in-app theme.
    @MainActor
    static var systemIsDark: Bool {
        let current = UITraitCollection.current.userInterfaceStyle
        let style = current == .unspecified ? UIScreen.main.traitCollection.userInterfaceStyle : current
        return style == .dark
    }

    /// Donates an INSendMessageIntent and returns the content the system
    /// draws with the sender's picture; nil when the intent is refused
    /// (missing entitlement), so callers keep the plain content.
    func communicationContent(from content: UNMutableNotificationContent, image: INImage?, groupImage: INImage? = nil) -> UNNotificationContent? {
        let handle = INPersonHandle(value: senderHandle.isEmpty ? senderId : senderHandle, type: .unknown)
        var components = PersonNameComponents()
        components.nickname = senderName
        let person = INPerson(personHandle: handle, nameComponents: components, displayName: senderName,
                              image: image, contactIdentifier: nil, customIdentifier: senderId, isMe: false, suggestionType: .none)
        let group = groupName.map { INSpeakableString(spokenPhrase: $0) }
        // The system draws group name / sender / content, so the content is the message alone;
        // the plain `body` ("Sender: text") stays for the fallback without an intent.
        let intent = INSendMessageIntent(recipients: nil, outgoingMessageType: .outgoingMessageText,
                                         content: group != nil ? (text ?? body) : body,
                                         speakableGroupName: group, conversationIdentifier: chatId,
                                         serviceName: "Arbos", sender: person, attachments: nil)
        if let image { intent.setImage(image, forParameterNamed: \.sender) }
        if group != nil, let conversation = groupImage ?? image {
            intent.setImage(conversation, forParameterNamed: \.speakableGroupName)
        }
        let interaction = INInteraction(intent: intent, response: nil)
        interaction.direction = .incoming
        interaction.donate { error in if let error { log.error("donate: \(error.localizedDescription, privacy: .public)") } }
        do {
            return try content.updating(from: intent)
        } catch {
            log.error("updating(from:) failed: \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }

    /// Plain content + the picture: what both the app and the extension deliver.
    @MainActor
    func makeContent(base: UNMutableNotificationContent) async -> UNNotificationContent {
        base.categoryIdentifier = messageCategory
        if base.threadIdentifier.isEmpty { base.threadIdentifier = chatId }
        if base.title.isEmpty { base.title = title }
        if base.body.isEmpty { base.body = body }
        var info = base.userInfo
        info["chat_id"] = chatId
        info["project_id"] = projectId
        base.userInfo = info
        let image = await senderImage()
        let conversation = await groupImage()
        log.notice("message for chat \(chatId, privacy: .public) from \(senderName, privacy: .public) kind=\(chatKind, privacy: .public) image=\(image == nil ? "none" : "ok", privacy: .public) group=\(conversation == nil ? "none" : "ok", privacy: .public)")
        return communicationContent(from: base, image: image, groupImage: conversation) ?? base
    }
}

/// What the notification extension needs to read an encrypted message without the app: which
/// Matrix device this phone is and the key of its store, in a keychain item the app and the
/// extension share through their App Group (the group is an access group of both; no other
/// entitlement). The store itself is under the group's container. A build without the group
/// keeps the store in the app's own container and every alert as the relay wrote it.
enum PreviewVault {
    static let group = "group.com.unarbos.arbos"

    struct Device: Codable, Equatable {
        var userId: String
        var deviceId: String
        var storeKey: String
    }

    /// Where the Matrix stores are, one directory per device id; nil without the App Group.
    static var storeRoot: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)?
            .appendingPathComponent("matrix", isDirectory: true)
    }

    private static let query: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "sh.arbos.ios.preview",
        kSecAttrAccount as String: "device",
        kSecAttrAccessGroup as String: group,
    ]

    static func load() -> Device? {
        guard storeRoot != nil else { return nil }
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess, let data = item as? Data else { return nil }
        return try? JSONDecoder().decode(Device.self, from: data)
    }

    /// Keep `device` for the extension, or forget it (signed out). Readable while the phone
    /// is locked, like the store's own key; never leaves this phone. Without the App Group
    /// there is no shared keychain to write and no extension that could read the store.
    static func save(_ device: Device?) {
        guard storeRoot != nil else { return }
        SecItemDelete(query as CFDictionary)
        guard let device, let data = try? JSONEncoder().encode(device) else { return }
        var add = query
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        if status != errSecSuccess { log.error("preview vault: keychain \(status)") }
    }
}
