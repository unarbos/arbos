// One agent, one picture, one name — on every surface (#1592, Jacob 2026-10-23:
// TestAgent's bubble drew a purple "T" while the working row drew its leaf and
// said "testagent"). The rule lives in arbos-core (`avatar::identity`) and reaches
// Swift through arbos-ffi (`resolveAgent`): a row's `sender` / `senderHandle` /
// `senderName` against the open chat's roster, then my projects, then every other
// roster; an agent no card knows gets the id-seeded leaf and the row's own name.
// This file only gathers the cards and turns the answer into the `Project` the
// avatar views draw — `AgentAvatar(project:)` everywhere, never a `PersonAvatar`
// seeded by a handle for an agent.

import Foundation
import ArbosFFI

extension AppModel {
    /// The cards an agent row is resolved against, in the rule's order: the open chat's
    /// roster (the relay's current truth for that chat), my projects, every other roster.
    func agentCards(chatId: Id?) -> [AgentCard] {
        var cards: [AgentCard] = []
        if let chatId, let chat = peopleChat(chatId) {
            cards += chat.agents.compactMap { Self.agentCard($0) }
        }
        cards += projects.map { Self.agentCard($0) }
        for chat in peopleChats where chat.id != chatId {
            cards += chat.agents.compactMap { Self.agentCard($0) }
        }
        return cards
    }

    static func agentCard(_ p: Project) -> AgentCard {
        AgentCard(
            id: p.id,
            name: p.name,
            handle: agentSlug(name: p.name),
            leaf: p.avatarLeaf.map { UInt8(clamping: $0) },
            leafColor: p.avatarLeafColor.map { UInt8(clamping: $0) },
            colorIndex: p.effectiveAvatarColorIndex.map { UInt8(clamping: $0) },
            imageKey: p.imageKey,
            avatarVersion: Int64(p.avatarVersion)
        )
    }

    static func agentCard(_ m: ChatMember) -> AgentCard? {
        guard m.isAgent else { return nil }
        guard let p = m.project else {
            return AgentCard(id: m.id, name: m.name ?? "", handle: m.handle, leaf: nil, leafColor: nil,
                             colorIndex: m.avatarColorIndex.map { UInt8(clamping: $0) }, imageKey: nil, avatarVersion: 0)
        }
        var card = agentCard(p)
        if !m.handle.isEmpty, m.handle != "agent" { card.handle = m.handle }
        return card
    }

    /// The shared resolver: which card a row is drawn from, and what every surface shows.
    func agentIdentity(sender: String, handle: String?, name: String?, chatId: Id?) -> AgentIdentity {
        resolveAgent(sender: sender, handle: handle, name: name, cards: agentCards(chatId: chatId))
    }

    /// The agent a people-chat row is drawn from, as every surface draws it: the roster's
    /// (or my) project record carrying the resolved name, leaf and picture; a stand-in
    /// `Project` when no card knows the agent (its id seeds the leaf, the row names it).
    func agent(sender: String, handle: String?, name: String?, chatId: Id?) -> Project {
        agentProject(agentIdentity(sender: sender, handle: handle, name: name, chatId: chatId), chatId: chatId)
    }

    /// A roster member through the same rule (its picture is the one its bubbles get).
    func agent(member m: ChatMember, chatId: Id?) -> Project {
        agent(sender: m.member, handle: m.handle, name: m.project?.name ?? m.name, chatId: chatId)
    }

    /// An agent by project id (call tiles, profile heroes, chips) through the same rule.
    func agent(id: Id, handle: String? = nil, name: String? = nil, chatId: Id? = nil) -> Project {
        agent(sender: id, handle: handle, name: name, chatId: chatId)
    }

    /// `AgentIdentity` → the `Project` the views draw. A known agent keeps its full record
    /// (model, state, owner…) with the identity's name, leaf and picture written over it.
    func agentProject(_ a: AgentIdentity, chatId: Id?) -> Project {
        // The resolver's `leaf` is the pair in effect (the automatic one for a neutral card, so
        // an older client still draws something); the stored neutral pair (Jacob, 2026-10-25)
        // is read off the card itself, so the initial-on-grey survives the round trip.
        let neutral = a.known && agentCards(chatId: chatId).first { $0.id == a.id }?.leaf == UInt8(clamping: LeafArt.neutral)
        var p = knownProject(a.id, chatId: chatId) ?? Project(
            id: a.id,
            name: a.name,
            icon: String(a.name.prefix(1)).uppercased(),
            color: "#8b7cf6",
            state: .awake,
            stateSince: nil,
            stateDetail: nil,
            model: "",
            effort: .low,
            spendTodayUsd: 0,
            spendCapUsd: 0,
            avatar: nil,
            avatarColor: nil
        )
        p.name = a.name
        // A neutral card (Create sheet, nothing picked — Jacob 2026-10-25) keeps its stored pair,
        // so `AgentAvatar` draws the initial on grey, never the automatic leaf.
        p.avatarLeaf = neutral ? LeafArt.neutral : Int(a.leaf.leaf)
        p.avatarLeafColor = neutral ? LeafArt.neutral : Int(a.leaf.color)
        p.avatarColorIndexEffective = Int(a.colorIndex)
        p.imageKey = a.imageKey
        p.imageURL = a.imageKey.flatMap { blobURL(projectId: a.id, key: $0)?.absoluteString }
        return p
    }

    /// The full record behind an id: mine first, then the roster rows (someone else's agent).
    private func knownProject(_ id: Id, chatId: Id?) -> Project? {
        if let mine = projects.first(where: { $0.id == id }) { return mine }
        let open: [PeopleChat] = chatId.flatMap { peopleChat($0) }.map { [$0] } ?? []
        let chats = open + peopleChats.filter { $0.id != chatId }
        for c in chats {
            if let a = c.agents.first(where: { $0.id == id }), var p = a.project {
                if p.ownerHandle == nil { p.ownerHandle = a.ownerHandle }
                if p.ownerId == nil { p.ownerId = a.ownerId }
                p.shared = true
                if p.description.isEmpty, let d = a.description { p.description = d }
                return p
            }
        }
        return nil
    }
}
