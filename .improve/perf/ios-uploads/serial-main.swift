// Historical serial send branches; full AppModel SHA256 e1451629c609367c1585653c119f7b5268141ea834d77689b06f04c057c28fd6
final class SerialSnapshot {
    func send(chatId: Id, text: String, attachments: [PendingAttachment] = [], replyTo: Id? = nil,
              forwardedFrom: String? = nil, extraParts: [[String: Any]] = []) {
        // A new send clears the composer's note (the next failure brings its own).
        clearSendError(chatId)
        if JobTabs.isJobChat(chatId) {
            steerFromJobTab(chatId: chatId, text: text)
            return
        }
        if attachments.isEmpty {
            write(chatId: chatId, text: text, command: "send", attachments: [], replyTo: replyTo,
                  forwardedFrom: forwardedFrom, extraParts: extraParts)
            return
        }
        guard let chat = chat(chatId) else { return }
        if isFixture {
            write(chatId: chatId, text: text, command: "send", attachments: attachments.map {
                Attachment(name: $0.name, mime: $0.mime, blobRef: nil, kind: Self.attachmentKind(mime: $0.mime, name: $0.name))
            }, replyTo: replyTo)
            return
        }
        Task {
            var uploaded: [Attachment] = []
            for a in attachments {
                let key = AppModel.blobKey(for: a.name)
                let kind = Self.attachmentKind(mime: a.mime, name: a.name)
                do {
                    _ = try await api.putBlob(projectId: chat.projectId, key: key, mime: a.mime, data: a.data)
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: key, kind: kind))
                } catch {
                    setSendError(chatId, UserError.classify("upload the attachment", error))
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: nil, kind: kind))
                }
            }
            // The forward header rides along with the pictures ([feedback
            // forward-attachments-ios-2026-10-27]: it was dropped on this path).
            write(chatId: chatId, text: text, command: "send", attachments: uploaded, replyTo: replyTo,
                  forwardedFrom: forwardedFrom, extraParts: extraParts)
        }
    }
    func sendPeople(chatId: String, text: String, replyTo: String?, address: String? = nil,
                    chipMentions: [String] = [], attachments: [PendingAttachment] = [],
                    forwardedFrom: String? = nil, chipTokens: [String: String] = [:],
                    replyRef: ReplyRef? = nil, extraParts: [[String: Any]] = []) {
        if let draft = draftDM, draft.id == chatId {
            sendDraftDM(draft) { [weak self] dmId in
                self?.sendPeople(chatId: dmId, text: text, replyTo: replyTo, address: address, chipMentions: chipMentions,
                                 attachments: attachments, forwardedFrom: forwardedFrom, chipTokens: chipTokens,
                                 replyRef: replyRef, extraParts: extraParts)
            }
            return
        }
        guard let doc = docs[chatId], let chat = peopleChat(chatId), let me = user else { return }
        var mentions = AppModel.mentions(in: text, members: chat.members, senderHandle: me.login, chipTokens: chipTokens)
        // Leading bare agent names ("Bob?", "hey Bob …", "Arbos what is your name") count as
        // addressing that agent — same as typing @bob — so the box wakes without `@`.
        // Resolve agents before people so a human display-name collision (e.g. @unarbos
        // named Arbos vs agent @arbos) does not steal the address.
        let leadingAgents = AppModel.namedAgents(in: text, members: chat.members)
        for agent in leadingAgents where !mentions.contains(agent) {
            mentions.append(agent)
        }
        // Leading bare person names ("Allan", "Allan?", "Yo Allan …") count as addressing
        // that person — so a sticky "Message @bob" chip does not wake the agent. When an
        // agent handle/slug already matched, only person @handles count (not display names).
        let leadingPeople = leadingAgents.isEmpty
            ? AppModel.namedPeople(in: text, members: chat.members)
            : AppModel.namedPeopleByHandle(in: text, members: chat.members)
        for person in leadingPeople where !mentions.contains(person) {
            mentions.append(person)
        }
        // Conversation-selector combinations: every selected agent is mentioned so each box
        // wakes (agentd allows several replies on a shared people `send`).
        for agent in chipMentions where agent.hasPrefix("agent:") && !mentions.contains(agent) {
            mentions.append(agent)
        }
        // Agent @mentions, bare agent names, and "everyone"/"agents" phrases count as an
        // address. A leading person name must not keep a sticky agent chip — otherwise
        // "Message @bob" + "Allan?" wakes bob. A person @handle mid-line must not block the
        // chip (bob still answers about @jo). "everyone" always expands to every agent even
        // when a leftover `@agent` chip already named one.
        let addressedAgent = mentions.contains { $0.hasPrefix("agent:") }
        if let address {
            if address == "everyone" {
                mentions.append(contentsOf: chat.agents.map(\.member).filter { !mentions.contains($0) })
            } else if !addressedAgent {
                if address.hasPrefix("agent:"), !leadingPeople.isEmpty {
                    // Text opens by naming a person; leave the sticky agent out of mentions.
                } else if chat.members.contains(where: { $0.member == address }), !mentions.contains(address) {
                    mentions.append(address)
                }
            }
        }
        // In an agent-only DM, carry on with the agent who last spoke when nothing addresses
        // an agent and the message is not aimed at a person. A DM with a human peer (Allan +
        // bob) must not inherit that sticky target — otherwise "New test" / "Don't respond bob"
        // still wakes bob. A person chip / @handle / bare name must not also ping the last
        // agent — otherwise "Message Allan" still wakes bob. In a group, leave the message
        // unaddressed (unless a chip / @agent set one) so the box decision model can choose
        // which agent replies from chat context.
        var reply = replyTo
        let addressedPerson = mentions.contains { $0.hasPrefix("user:") }
        let hasHumanPeer = chat.humans.contains { $0.id != me.id }
        if chat.kind != "group", !hasHumanPeer, !mentions.contains(where: { $0.hasPrefix("agent:") }),
           !addressedPerson, reply == nil,
           let last = messages(of: chatId).last(where: { $0.role == .assistant }) {
            reply = last.id
            if let h = last.senderHandle?.lowercased(),
               let agent = chat.agents.first(where: {
                   mentionSlug($0.project?.name ?? "").lowercased() == h || $0.handle.lowercased() == h
               }) {
                mentions.append(agent.member)
            }
        }
        let author = CoreChatDoc.Author(sender: "user:\(me.id)", handle: me.login, name: me.name, avatar: me.avatarUrl)
        let messageId = newId()
        // Text-only: write on this turn (same as project `write`) so the push leaves
        // before we return. Deferring every send into a Task left bubbles faint when
        // the room ACK raced a still-queued commit.
        // A cross-chat reply never carries an in-chat `replyTo` as well.
        if replyRef != nil { reply = nil }
        if attachments.isEmpty {
            commitPeople(chatId: chatId, doc: doc, messageId: messageId, text: text, author: author,
                         mentions: mentions, reply: reply, attachments: [], forwardedFrom: forwardedFrom,
                         replyRef: replyRef, extraParts: extraParts)
            return
        }
        markUnacked(chatId: chatId, messageId: messageId)
        Task {
            var uploaded: [Attachment] = []
            for a in attachments {
                let key = AppModel.blobKey(for: a.name)
                do {
                    _ = try await api.putBlob(projectId: "\(peopleSlot):\(chatId)", key: key, mime: a.mime, data: a.data)
                    uploaded.append(Attachment(name: a.name, mime: a.mime, blobRef: key, kind: Self.attachmentKind(mime: a.mime, name: a.name)))
                } catch {
                    setSendError(chatId, UserError.classify("upload the attachment", error))
                }
            }
            // The forward header rides along with the pictures ([feedback
            // forward-attachments-ios-2026-10-27]: it was dropped on this path).
            commitPeople(chatId: chatId, doc: doc, messageId: messageId, text: text, author: author,
                         mentions: mentions, reply: reply, attachments: uploaded, alreadyUnacked: true,
                         forwardedFrom: forwardedFrom, replyRef: replyRef, extraParts: extraParts)
        }
    }
}
