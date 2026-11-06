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
        let cards = agentCards(chatId: chatId)
        let resolved = resolveAgent(sender: sender, handle: handle, name: name, cards: cards)
        if resolved.known { return resolved }
        // A live row the box has not attributed yet (`sender: "agent"`, no handle, no name)
        // in a people chat with one agent is that agent — the same rule as the Mac (Jacob,
        // 2026-10-26: the working row drew a seeded leaf while the finished bubbles had the
        // photo). Several agents on the roster: nothing is safe to assume.
        let bare = agentSenderId(sender: sender).isEmpty || sender == "agent" || sender == "assistant"
        if bare, (handle ?? "").trimmingCharacters(in: .whitespaces).isEmpty,
           let chatId, let chat = peopleChat(chatId), chat.agents.count == 1 {
            let only = chat.agents[0]
            return resolveAgent(sender: only.id, handle: only.handle, name: only.project?.name ?? only.name, cards: cards)
        }
        return resolved
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
        p.avatarVersion = Int(a.avatarVersion)
        p.imageURL = agentPictureURL(id: a.id, imageKey: a.imageKey, version: p.avatarVersion)
        return p
    }

    // MARK: One copy of the avatar everywhere (Jacob, 2026-10-25)

    /// The agent's picture URL as every surface fetches it (`Project.pictureURL`); nil in the
    /// fixture, where there is no relay to serve it.
    func agentPictureURL(id: Id, imageKey: String?, version: Int) -> String? {
        guard !isFixture else { return nil }
        return Project.pictureURL(httpUrl: relayHttpUrl, id: id, imageKey: imageKey, version: version)
    }

    /// What an agent's avatar is made of, with the relay's stamp of its last change.
    struct AgentAvatarFacts: Equatable {
        var imageKey: String?
        var avatarLeaf: Int?
        var avatarLeafColor: Int?
        var version: Int

        init(imageKey: String?, avatarLeaf: Int?, avatarLeafColor: Int?, version: Int) {
            self.imageKey = imageKey.flatMap { $0.isEmpty ? nil : $0 }
            self.avatarLeaf = avatarLeaf
            self.avatarLeafColor = avatarLeafColor
            self.version = version
        }

        init(_ p: Project) {
            self.init(imageKey: p.imageKey, avatarLeaf: p.avatarLeaf, avatarLeafColor: p.avatarLeafColor, version: p.avatarVersion)
        }
    }

    /// Every copy of an agent the model holds: my project record and the roster card of each
    /// people chat it sits on. These used to drift apart after a change (only the copy the
    /// change went through moved), so the header drew one picture and the bubbles another.
    func agentAvatarCopies(_ projectId: Id) -> [AgentAvatarFacts] {
        var out: [AgentAvatarFacts] = []
        if let mine = projects.first(where: { $0.id == projectId }) { out.append(AgentAvatarFacts(mine)) }
        for chat in peopleChats {
            for m in chat.agents where m.id == projectId {
                if let p = m.project { out.append(AgentAvatarFacts(p)) }
            }
        }
        return out
    }

    /// The newest facts the model knows for an agent: the highest stamp wins; among equal
    /// stamps the first copy (my record, then the rosters in list order). The stamp is the only
    /// truth — never "the one with a picture": that guess made a stale picture beat a reset
    /// whose stamp tied (Jacob, 2026-10-26). The relay stamps every row, so two differing
    /// states cannot share a stamp any more.
    func newestAgentAvatar(_ projectId: Id) -> AgentAvatarFacts? {
        agentAvatarCopies(projectId).reduce(nil) { best, c in
            guard let b = best else { return c }
            return c.version > b.version ? c : b
        }
    }

    /// Write `facts` into every copy of the agent — the one apply path. A copy that already
    /// carries a newer stamp is left alone (a late answer never rolls a picture back); `force`
    /// is for the optimistic local write before the relay has stamped the change.
    func applyAgentAvatar(_ projectId: Id, _ facts: AgentAvatarFacts, force: Bool = false) {
        func write(_ p: inout Project) {
            guard force || facts.version >= p.avatarVersion else { return }
            p.imageKey = facts.imageKey
            p.avatarLeaf = facts.avatarLeaf
            p.avatarLeafColor = facts.avatarLeafColor
            p.avatarVersion = facts.version
            p.imageURL = agentPictureURL(id: projectId, imageKey: facts.imageKey, version: facts.version)
        }
        // Write through `@Published` only on a change: an `inout` pass publishes even when
        // `write` leaves the copy as it was, and every poll reconciles every agent.
        for i in projects.indices where projects[i].id == projectId {
            var p = projects[i]
            write(&p)
            if p != projects[i] { projects[i] = p }
        }
        for c in peopleChats.indices {
            for m in peopleChats[c].members.indices where peopleChats[c].members[m].isAgent && peopleChats[c].members[m].id == projectId {
                guard var p = peopleChats[c].members[m].project else { continue }
                write(&p)
                if p != peopleChats[c].members[m].project { peopleChats[c].members[m].project = p }
            }
        }
    }

    /// After any fetch that replaced a copy (registry, people list): per agent, the newest copy
    /// wins everywhere, so a roster that arrived with an older picture cannot flip a surface back.
    func reconcileAgentAvatars() {
        var ids = Set(projects.map(\.id))
        for chat in peopleChats { for m in chat.agents { ids.insert(m.id) } }
        for id in ids {
            guard let newest = newestAgentAvatar(id) else { continue }
            applyAgentAvatar(id, newest)
        }
    }

    /// The picture only (the push carries no leaf picks): a newer stamp from the relay lands in
    /// place at once; the leaf picks come with the re-read that follows.
    func applyAgentAvatarPush(projectId: Id, imageKey: String?, version: Int) {
        let base = newestAgentAvatar(projectId) ?? AgentAvatarFacts(imageKey: nil, avatarLeaf: nil, avatarLeafColor: nil, version: 0)
        guard version >= base.version else { return }
        applyAgentAvatar(projectId, AgentAvatarFacts(imageKey: imageKey, avatarLeaf: base.avatarLeaf, avatarLeafColor: base.avatarLeafColor, version: version))
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
