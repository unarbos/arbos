// Package messenger is the Telegram door onto the kernel. Each registered bot
// token is a standing inbox; each Telegram chat that messages it maps to one
// durable kernel session — the same session the web UI opens as a full chat
// tab. The bridge owns no conversation state of its own: inbound messages
// become PromptIntents (run on the bridge's ephemeral actor, or injected into
// the web tab's live actor when one is attached), and outbound delivery is a
// tail on the session's event log — whatever lands there, from either door,
// mirrors to the Telegram thread. One log, two doors.
//
// Trust is per bot: the Tools toggle decides whether its turns run on the
// host's own engine (everything a normal chat can do) or a chat-only engine
// with an empty toolset. Every bot locks to its first sender.
package messenger

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/outbox"
)

// turnTimeout bounds one bridge-run turn (mirrors the plan scheduler's wake
// timeout); on expiry the turn is interrupted, not abandoned.
const turnTimeout = 15 * time.Minute

// convoQueueDepth bounds messages waiting on a chat's worker while a turn
// runs; beyond it the bridge tells the sender it is overloaded.
const convoQueueDepth = 16

// tailInterval is how often the mirror tail polls bridged sessions for new
// events (cheap — an indexed seq-range read per chat). One second keeps the
// phone close behind the log; turn completions also kick the tail directly.
const tailInterval = time.Second

// typingRefresh is how often the tail re-sends the typing indicator while a
// turn is in flight (Telegram clears it after ~5s).
const typingRefresh = 3 * time.Second

// originTelegram marks bridge-injected prompts so (a) an attached web tab
// echoes them visibly and (b) the tail recognizes its own injections.
const originTelegram = "telegram"

// toollessSystemPrompt frames the chat-only engine that serves bots whose
// tools toggle is off.
const toollessSystemPrompt = `You are arbos, a personal AI agent, chatting over Telegram.

Tools are switched off for this connector: no file access, no shell, no web. Do not pretend otherwise — if asked to do something that needs them, say tools are disabled here and offer what you can do in conversation instead.

Replies are delivered as plain-text Telegram messages: keep them concise and skip heavy markdown.`

// BotConfig is one registered bot, as persisted (the token never crosses the
// wire back out — see BotView).
type BotConfig struct {
	ID       int64  `json:"id"`
	Username string `json:"username"`
	Token    string `json:"token"`
	// Tools is the connector's one permission switch: on (the default) the
	// bot's turns run on the host's own engine — everything a normal chat
	// can do; off they run on a chat-only engine with an empty toolset.
	Tools bool `json:"tools"`
	// BoundUser locks the bot to the Telegram user who messaged it first —
	// bot usernames are publicly searchable, and an unbound bot would hand
	// this arbos to whoever finds it.
	BoundUser int64  `json:"bound_user,omitempty"`
	BoundName string `json:"bound_name,omitempty"`
	AddedAt   int64  `json:"added_at"`
	// LegacyAccess decodes the pre-toggle "access" field ("full"/"guest");
	// load() migrates it onto Tools and clears it.
	LegacyAccess string `json:"access,omitempty"`
}

// BotView is the panel-facing shape of a bot — everything but the token.
type BotView struct {
	ID        int64  `json:"id"`
	Username  string `json:"username"`
	Tools     bool   `json:"tools"`
	BoundName string `json:"bound_name,omitempty"`
	AddedAt   int64  `json:"added_at"`
}

func (b BotConfig) view() BotView {
	return BotView{ID: b.ID, Username: b.Username, Tools: b.Tools, BoundName: b.BoundName, AddedAt: b.AddedAt}
}

// Convo is one Telegram chat bridged to one kernel session.
type Convo struct {
	ID        string `json:"id"` // "<botID>:<chatID>"
	BotID     int64  `json:"bot_id"`
	ChatID    int64  `json:"chat_id"`
	Title     string `json:"title"`
	Kind      string `json:"kind"` // private | group | supergroup
	SessionID string `json:"session_id"`
	LastAt    int64  `json:"last_at,omitempty"`
	// NextSeq is the mirror tail's cursor into the session's event log: the
	// seq of the next event not yet delivered to Telegram. Persisted so a
	// restart never re-blasts history.
	NextSeq int64 `json:"next_seq,omitempty"`
	// Busy is wire-only: a bridge-run turn is in flight.
	Busy bool `json:"busy,omitempty"`
}

// Event is one frame on the panel's stream. State carries a full snapshot
// (sent on subscribe and on registry changes); conversation is an upsert.
// Bots and Conversations carry no omitempty: a state frame's empty registry
// must arrive as [] for the panel, never vanish from the JSON.
type Event struct {
	Type          string    `json:"type"` // state | conversation
	Bots          []BotView `json:"bots"`
	Conversations []Convo   `json:"conversations"`
	Conversation  *Convo    `json:"conversation,omitempty"`
}

// KernelStore is the bridge's read on the durable store, satisfied by
// *sqlite.Store: the event-log tail (EventsFrom returns events with
// Seq >= from, oldest first) and the outbox claim that makes the phone a
// delivery door for the agent's between-turn voice.
type KernelStore interface {
	EventsFrom(ctx context.Context, id core.SessionID, from int64) ([]core.Event, error)
	ClaimOutboxFor(ctx context.Context, via, principal string, sessions []string, staleBefore time.Time) ([]outbox.Message, error)
}

// viaTelegram is the outbox delivered_via marker for this door.
const viaTelegram = "telegram"

// outboxPoll is the cadence of outbox claims (mirrors the gateway's).
const outboxPoll = 3 * time.Second

// Config wires a Service.
type Config struct {
	// Dir is where the bot registry and conversation index persist
	// (state.json, mode 0600 — it holds bot tokens).
	Dir string
	// Full is the host's own engine; tools-on turns run here, and an open
	// web tab's actor (always on this engine) receives injected prompts.
	Full *engine.Engine
	// Guest builds the chat-only engine for tools-off bots, once, on first
	// need (piwire.Host.GuestEngine with the bridge's prompt).
	Guest func(systemPrompt string) *engine.Engine
	// Store is the durable event log the mirror tail reads. Required.
	Store KernelStore
	// Transcribe converts an inbound voice memo (base64 + container format)
	// to text; nil leaves voice messages as a placeholder.
	Transcribe func(ctx context.Context, dataB64, format string) (string, error)
	// Commands lists the host's slash commands (prompt templates / skills),
	// read live so a freshly added template reaches the phone without a
	// restart. They are published to each bot's Telegram "/" menu, and
	// inbound commands are mapped back to their template names (Telegram
	// only allows [a-z0-9_], so "poteto-mode" travels as "/poteto_mode").
	// nil disables the menu; bare /commands still pass through as text.
	Commands func() []Command
	// Logf receives bridge diagnostics; nil discards.
	Logf func(format string, args ...any)
}

// Command is one slash command offered on the Telegram menu.
type Command struct {
	Name        string
	Description string
}

// Service owns the bot runners, the chat workers, the mirror tail, and the
// panel fan-out.
type Service struct {
	cfg Config
	ctx context.Context // service lifetime, set by Start
	// kick nudges the mirror tail ahead of its tick — a completed
	// bridge-run turn delivers its reply immediately, not a poll later.
	kick chan struct{}

	mu       sync.Mutex
	bots     map[int64]*botRunner
	convos   map[string]*convo
	guestEng *engine.Engine
	subs     map[chan Event]bool
}

type botRunner struct {
	cfg    BotConfig
	client *tgClient
	// ctx scopes the bot's poll loop and chat workers; cancel is RemoveBot's
	// teardown of all of them at once.
	ctx    context.Context
	cancel context.CancelFunc
}

type convo struct {
	c     Convo
	queue chan inbound
	// pending holds prompt texts this bridge injected itself, so the tail
	// can tell its own injections (already on the phone) from prompts that
	// arrived through other doors (mirror those). Guarded by Service.mu.
	pending []string
	// prime marks a conversation loaded from a pre-tail state file: its
	// first tail pass fast-forwards past existing history instead of
	// re-delivering the whole conversation to Telegram.
	prime bool
	// inFlight is the tail's read on whether a turn is mid-run (the last
	// log event was a prompt or tool activity, not a final reply) — it
	// drives the Telegram typing indicator for turns the bridge didn't run
	// itself (the web tab's). inFlightAt bounds a stuck flag; lastTyping
	// throttles the refresh. All guarded by Service.mu.
	inFlight   bool
	inFlightAt time.Time
	lastTyping time.Time
	// streamed records text segments already live-streamed to the phone
	// (send-early-edit-often), so the tail skips them when the same text
	// lands in the log. Guarded by Service.mu.
	streamed []string
	// ask is the QuestionRequest a suspended turn is waiting on, presented
	// on the phone and awaiting the owner's next reply (see ask.go).
	// Guarded by Service.mu.
	ask *pendingAsk
	// stream is the live pipeline currently consuming a tapped web-tab
	// actor for this chat, if any; follow-up prompts reuse it rather than
	// stacking a second tap (see stream.go). Guarded by Service.mu.
	stream *turnStream
}

// inbound is one Telegram message, reduced to what a turn needs.
type inbound struct {
	from  tgUser
	text  string
	photo string // file_id of the largest photo size, if any
	voice *tgVoice
	doc   *tgDocument
	at    time.Time
}

// New loads the persisted registry; Start brings it to life.
func New(cfg Config) (*Service, error) {
	if cfg.Store == nil {
		return nil, errors.New("messenger: Config.Store is required")
	}
	s := &Service{
		cfg:    cfg,
		kick:   make(chan struct{}, 1),
		bots:   map[int64]*botRunner{},
		convos: map[string]*convo{},
		subs:   map[chan Event]bool{},
	}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

// Start launches a poller per registered bot, a worker per known chat, and
// the mirror tail. Blocks nothing; everything lives until ctx is cancelled.
func (s *Service) Start(ctx context.Context) {
	s.mu.Lock()
	s.ctx = ctx
	for _, b := range s.bots {
		s.startBotLocked(b)
	}
	s.mu.Unlock()
	go s.runTail(ctx)
	go s.runOutbox(ctx)
}

func (s *Service) logf(format string, args ...any) {
	if s.cfg.Logf != nil {
		s.cfg.Logf(format, args...)
	}
}

// --- registry ---------------------------------------------------------------

// AddBot validates the token against Telegram, persists the bot, and starts
// polling it immediately.
func (s *Service) AddBot(ctx context.Context, token string, tools bool) (BotView, error) {
	me, err := newTGClient(token).getMe(ctx)
	if err != nil {
		return BotView{}, fmt.Errorf("token rejected by Telegram: %w", err)
	}
	cfg := BotConfig{
		ID: me.ID, Username: me.Username, Token: token,
		Tools: tools, AddedAt: time.Now().UnixMilli(),
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if old := s.bots[me.ID]; old != nil {
		old.cancel()
	}
	b := &botRunner{cfg: cfg, client: newTGClient(token)}
	s.bots[me.ID] = b
	s.startBotLocked(b)
	s.saveLocked()
	s.broadcastStateLocked()
	return cfg.view(), nil
}

// SetTools flips a bot's permission switch; it applies from the next
// bridge-run turn (the engine is chosen per turn).
func (s *Service) SetTools(id int64, tools bool) (BotView, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	b := s.bots[id]
	if b == nil {
		return BotView{}, fmt.Errorf("no bot %d", id)
	}
	b.cfg.Tools = tools
	s.saveLocked()
	s.broadcastStateLocked()
	return b.cfg.view(), nil
}

// RemoveBot stops the bot's poller and chat workers and drops it and its
// conversations from the registry. The kernel sessions remain in the store —
// re-adding the same bot resumes them.
func (s *Service) RemoveBot(id int64) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	b := s.bots[id]
	if b == nil {
		return fmt.Errorf("no bot %d", id)
	}
	b.cancel()
	delete(s.bots, id)
	for key, cv := range s.convos {
		if cv.c.BotID == id {
			delete(s.convos, key)
		}
	}
	s.saveLocked()
	s.broadcastStateLocked()
	return nil
}

// State snapshots the registry for the panel's initial render.
func (s *Service) State() Event {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.stateLocked()
}

func (s *Service) stateLocked() Event {
	ev := Event{Type: "state", Bots: []BotView{}, Conversations: []Convo{}}
	for _, b := range s.bots {
		ev.Bots = append(ev.Bots, b.cfg.view())
	}
	for _, cv := range s.convos {
		ev.Conversations = append(ev.Conversations, cv.c)
	}
	return ev
}

// Subscribe attaches a panel to the event stream; the snapshot arrives as the
// first frame. A slow subscriber drops frames rather than stalling the bridge.
func (s *Service) Subscribe() (<-chan Event, func()) {
	ch := make(chan Event, 64)
	s.mu.Lock()
	s.subs[ch] = true
	ch <- s.stateLocked()
	s.mu.Unlock()
	return ch, func() {
		s.mu.Lock()
		delete(s.subs, ch)
		s.mu.Unlock()
	}
}

func (s *Service) broadcastLocked(ev Event) {
	for ch := range s.subs {
		select {
		case ch <- ev:
		default:
		}
	}
}

func (s *Service) broadcastConvo(cv *convo) {
	s.mu.Lock()
	c := cv.c
	s.broadcastLocked(Event{Type: "conversation", Conversation: &c})
	s.mu.Unlock()
}

func (s *Service) broadcastStateLocked() {
	s.broadcastLocked(s.stateLocked())
}

// --- persistence -------------------------------------------------------------

type stateFile struct {
	Bots   []BotConfig `json:"bots"`
	Convos []Convo     `json:"conversations"`
}

func (s *Service) statePath() string { return filepath.Join(s.cfg.Dir, "state.json") }

func (s *Service) load() error {
	b, err := os.ReadFile(s.statePath())
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var st stateFile
	if err := json.Unmarshal(b, &st); err != nil {
		return fmt.Errorf("messenger state: %w", err)
	}
	for _, cfg := range st.Bots {
		// Migrate the pre-toggle "access" tiers: "guest" meant no tools.
		if cfg.LegacyAccess != "" {
			cfg.Tools = cfg.LegacyAccess != "guest"
			cfg.LegacyAccess = ""
		}
		s.bots[cfg.ID] = &botRunner{cfg: cfg, client: newTGClient(cfg.Token)}
	}
	for _, c := range st.Convos {
		c.Busy = false
		s.convos[c.ID] = &convo{
			c:     c,
			queue: make(chan inbound, convoQueueDepth),
			// A conversation that has never been tailed (pre-tail state
			// file) fast-forwards past its existing history.
			prime: c.NextSeq == 0,
		}
	}
	return nil
}

// saveLocked writes the registry atomically; tokens live here, hence 0600.
func (s *Service) saveLocked() {
	st := stateFile{Bots: []BotConfig{}, Convos: []Convo{}}
	for _, b := range s.bots {
		st.Bots = append(st.Bots, b.cfg)
	}
	for _, cv := range s.convos {
		st.Convos = append(st.Convos, cv.c)
	}
	b, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		s.logf("messenger: encode state: %v", err)
		return
	}
	if err := os.MkdirAll(s.cfg.Dir, 0o700); err != nil {
		s.logf("messenger: state dir: %v", err)
		return
	}
	tmp := s.statePath() + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		s.logf("messenger: write state: %v", err)
		return
	}
	if err := os.Rename(tmp, s.statePath()); err != nil {
		s.logf("messenger: write state: %v", err)
	}
}

// --- polling ------------------------------------------------------------------

// startBotLocked launches the bot's long-poll loop and a worker per known
// chat, all scoped to a per-bot context so RemoveBot tears everything down.
func (s *Service) startBotLocked(b *botRunner) {
	b.ctx, b.cancel = context.WithCancel(s.ctx)
	for _, cv := range s.convos {
		if cv.c.BotID == b.cfg.ID {
			go s.runConvo(b.ctx, b, cv)
		}
	}
	go s.runBot(b.ctx, b)
	go s.publishCommands(b.ctx, b)
}

// publishCommands puts the bridge's built-ins and the host's slash commands
// on the bot's Telegram "/" menu, so skills are one keystroke away on the
// phone too. Best-effort: a failure costs the menu, never the bridge.
func (s *Service) publishCommands(ctx context.Context, b *botRunner) {
	// The control built-ins lead the menu (and shadow same-named templates;
	// routeTurn intercepts them before template expansion).
	out := []tgBotCommand{
		{Command: "stop", Description: "Stop the turn that is running"},
		{Command: "steer", Description: "Redirect the running turn: /steer <new instruction>"},
	}
	seen := map[string]bool{"stop": true, "steer": true}
	var cmds []Command
	if s.cfg.Commands != nil {
		cmds = s.cfg.Commands()
	}
	for _, c := range cmds {
		name := tgCommandName(c.Name)
		if name == "" || seen[name] {
			continue
		}
		seen[name] = true
		desc := strings.TrimSpace(c.Description)
		if desc == "" {
			desc = "/" + c.Name
		}
		if len(desc) > 256 {
			desc = desc[:256]
		}
		out = append(out, tgBotCommand{Command: name, Description: desc})
		if len(out) == 100 { // Telegram's menu cap
			break
		}
	}
	if len(out) == 0 {
		return
	}
	if err := b.client.setMyCommands(ctx, out); err != nil {
		s.logf("messenger: @%s commands menu: %v", b.cfg.Username, err)
	}
}

// tgCommandName maps a template name onto Telegram's command alphabet
// ([a-z0-9_], max 32): lowercase, dashes become underscores, anything else
// drops. Empty means the name can't be represented.
func tgCommandName(name string) string {
	var sb strings.Builder
	for _, r := range strings.ToLower(name) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '_':
			sb.WriteRune(r)
		case r == '-':
			sb.WriteByte('_')
		}
	}
	out := sb.String()
	if len(out) > 32 {
		out = out[:32]
	}
	return out
}

// resolveCommand maps an inbound Telegram command back onto the template it
// names: "/poteto_mode@MyBot rest" becomes "/poteto-mode rest". Text that
// isn't a command, or names no known template, passes through unchanged
// (the engine's expander ignores unknown slashes the same way).
func (s *Service) resolveCommand(text, botUsername string) string {
	if !strings.HasPrefix(text, "/") {
		return text
	}
	head, rest, _ := strings.Cut(text, " ")
	// Group clients disambiguate with "/cmd@BotName"; strip our own suffix.
	if cmd, at, ok := strings.Cut(head, "@"); ok {
		if !strings.EqualFold(at, botUsername) {
			return text // someone else's command
		}
		head = cmd
	}
	if s.cfg.Commands == nil {
		return joinCommand(head, rest)
	}
	typed := strings.TrimPrefix(head, "/")
	for _, c := range s.cfg.Commands() {
		if tgCommandName(c.Name) == strings.ToLower(typed) || strings.EqualFold(c.Name, typed) {
			return joinCommand("/"+c.Name, rest)
		}
	}
	return joinCommand(head, rest)
}

func joinCommand(head, rest string) string {
	if rest == "" {
		return head
	}
	return head + " " + rest
}

func (s *Service) runBot(ctx context.Context, b *botRunner) {
	var offset int64
	for ctx.Err() == nil {
		ups, err := b.client.getUpdates(ctx, offset)
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			s.logf("messenger: @%s poll: %v", b.cfg.Username, err)
			select {
			case <-ctx.Done():
				return
			case <-time.After(5 * time.Second):
			}
			continue
		}
		for _, u := range ups {
			offset = u.UpdateID + 1
			if u.Message == nil || u.Message.From == nil {
				continue
			}
			s.inbound(ctx, b, *u.Message)
		}
	}
}

// inbound routes one Telegram message: enforce the bot's binding, find or
// create the chat's conversation, and hand it to the chat's worker.
func (s *Service) inbound(ctx context.Context, b *botRunner, m tgMessage) {
	in := inbound{from: *m.From, text: m.Text, voice: m.Voice, doc: m.Document, at: time.Now()}
	if in.text == "" {
		in.text = m.Caption
	}
	if len(m.Photo) > 0 {
		in.photo = m.Photo[len(m.Photo)-1].FileID // largest size is last
	}
	if m.Sticker != nil && in.text == "" {
		in.text = "[sticker]"
	}
	if in.text == "" && in.photo == "" && in.voice == nil && in.doc == nil {
		return
	}

	s.mu.Lock()
	// Every bot locks to whoever messages it first — usually its creator,
	// or the friend it was handed to. Bot usernames are publicly searchable,
	// so without the lock a connector would answer whoever finds it.
	if b.cfg.BoundUser == 0 {
		b.cfg.BoundUser = in.from.ID
		b.cfg.BoundName = in.from.name()
		s.saveLocked()
		s.broadcastStateLocked()
		s.logf("messenger: @%s bound to %s", b.cfg.Username, in.from.name())
	} else if b.cfg.BoundUser != in.from.ID {
		s.mu.Unlock()
		_ = b.client.sendMessage(ctx, m.Chat.ID, "This arbos is private.")
		return
	}

	key := fmt.Sprintf("%d:%d", b.cfg.ID, m.Chat.ID)
	cv := s.convos[key]
	if cv == nil {
		cv = &convo{
			c: Convo{
				ID: key, BotID: b.cfg.ID, ChatID: m.Chat.ID,
				Title: m.Chat.title(), Kind: m.Chat.Type,
				SessionID: fmt.Sprintf("tg-%d-%d", b.cfg.ID, m.Chat.ID),
			},
			queue: make(chan inbound, convoQueueDepth),
		}
		s.convos[key] = cv
		s.saveLocked()
		s.broadcastStateLocked()
		go s.runConvo(b.ctx, b, cv)
	}
	s.mu.Unlock()

	select {
	case cv.queue <- in:
	default:
		_ = b.client.sendMessage(ctx, m.Chat.ID, "(arbos is overloaded — message dropped, try again shortly)")
	}
}

// --- inbound turns --------------------------------------------------------------

// runConvo is a chat's single worker: it serializes turns so two Telegram
// messages can never race one kernel session. handleInbound returns the
// messages that arrived mid-turn and were neither answers nor control
// commands — they run next, in arrival order, before anything newer.
func (s *Service) runConvo(ctx context.Context, b *botRunner, cv *convo) {
	for {
		select {
		case <-ctx.Done():
			return
		case msg := <-cv.queue:
			backlog := []inbound{msg}
			for len(backlog) > 0 && ctx.Err() == nil {
				next := backlog[0]
				backlog = append(backlog[1:], s.handleInbound(ctx, b, cv, next)...)
			}
		}
	}
}

// engineFor resolves the tools toggle to an engine, building the shared
// chat-only engine on first use.
func (s *Service) engineFor(tools bool) *engine.Engine {
	if tools {
		return s.cfg.Full
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.guestEng == nil {
		s.guestEng = s.cfg.Guest(toollessSystemPrompt)
	}
	return s.guestEng
}

// botTools reads a bot's live toggle (it can flip between turns).
func (s *Service) botTools(b *botRunner) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return b.cfg.Tools
}

// handleInbound turns one Telegram message into a kernel prompt and sees the
// turn run: on the bridge's own ephemeral actor when the session is free, or
// injected into the door that holds it (an open web tab) otherwise. While the
// turn runs, its deltas live-stream back to the phone (send-early-edit-often);
// anything the streamer didn't deliver, the mirror tail still will. It
// returns the messages that arrived while its own turn ran and were plain
// conversation — the caller runs them next, in order.
func (s *Service) handleInbound(ctx context.Context, b *botRunner, cv *convo, msg inbound) []inbound {
	// The control surface first: /stop, /steer, and a pending question's
	// answer act on the in-flight turn instead of becoming prompts.
	if s.routeControl(ctx, b, cv, &msg) {
		return nil
	}
	intent, err := s.buildPrompt(ctx, b, cv, msg)
	if err != nil {
		s.logf("messenger: @%s prompt: %v", b.cfg.Username, err)
		_ = b.client.sendMessage(ctx, cv.c.ChatID, "(could not read that message: "+err.Error()+")")
		return nil
	}

	// Remember our own injection so the tail doesn't mirror it back to the
	// phone that just sent it.
	s.mu.Lock()
	cv.pending = append(cv.pending, intent.Text)
	cv.c.LastAt = msg.at.UnixMilli()
	s.mu.Unlock()
	s.broadcastConvo(cv)

	sessID := core.SessionID(cv.c.SessionID)
	if engine.Sessions.Acquire(sessID) {
		ts := s.startStream(ctx, b, cv, sessID, nil)
		backlog := s.runOwnTurn(ctx, b, cv, sessID, intent, ts.push)
		// The reply is in the log; deliver it now, not a poll later — but
		// only after the consumer has sealed the streamer and recorded what
		// it delivered, or the tail re-sends the final reply it cannot yet
		// recognize as already streamed.
		ts.finish()
		s.kickTail()
		return backlog
	}

	// Another door holds the session: an open web tab's actor, always on
	// the host's own engine. A tools-off bot stops here — injecting would
	// run its turn with the full toolset (shell, files), the exact
	// escalation its toggle exists to prevent. Tools-on bots hand the
	// prompt to that actor (Origin makes the tab echo it) and stream the
	// turn back through a tap (see injectTurn).
	if s.botTools(b) {
		if conv := s.cfg.Full.Live(sessID); conv != nil && s.injectTurn(ctx, b, cv, sessID, conv, intent) {
			return nil
		}
	}
	// The prompt never landed: drop its pending entry, or a later identical
	// web-tab message would be silently swallowed instead of mirrored.
	s.consumePending(cv, intent.Text)
	_ = b.client.sendMessage(ctx, cv.c.ChatID, "(arbos is busy in this conversation — try again in a moment)")
	return nil
}

// routeControl is the handleInbound half of the control surface: it acts on
// whatever live actor currently holds this session (an open web tab's, or
// none). The bridge's own turns never reach here mid-flight — runOwnTurn
// drains the queue itself and routes through the same routeTurn. Tools-off
// bots get no actor handle: steering or answering a full-engine turn is the
// same escalation as injecting prompts into it.
func (s *Service) routeControl(ctx context.Context, b *botRunner, cv *convo, msg *inbound) bool {
	var conv *engine.Conversation
	if s.botTools(b) {
		conv = s.cfg.Full.Live(core.SessionID(cv.c.SessionID))
	}
	return s.routeTurn(ctx, b, cv, conv, msg)
}

// routeTurn intercepts the messages that act on an in-flight turn rather than
// becoming prompts: /stop interrupts it, /steer replaces it (the engine
// cancels the current turn silently and runs the new text), and a reply while
// a question is pending answers the question — so a turn suspended on the ask
// tool can always be unblocked, redirected, or killed from the phone. These
// built-ins shadow any prompt templates of the same names. conv is the live
// actor to act on; nil means nothing is running (then /steer degrades to a
// plain prompt by rewriting the message). Returns true when the message was
// consumed; false leaves it to the normal prompt path.
func (s *Service) routeTurn(ctx context.Context, b *botRunner, cv *convo, conv *engine.Conversation, msg *inbound) bool {
	head, rest, _ := strings.Cut(strings.TrimSpace(msg.text), " ")
	rest = strings.TrimSpace(rest)
	// Group clients disambiguate with "/cmd@BotName"; strip our own suffix.
	if cmd, at, ok := strings.Cut(head, "@"); ok && strings.EqualFold(at, b.cfg.Username) {
		head = cmd
	}
	switch strings.ToLower(head) {
	case "/stop":
		s.clearAsk(cv)
		if conv == nil {
			_ = b.client.sendMessage(ctx, cv.c.ChatID, "(nothing is running)")
			return true
		}
		// Urgent: never let a full intent buffer drop the interrupt
		// (mirrors Engine.Interrupt's delivery).
		if !conv.TrySend(core.InterruptIntent{}) {
			go conv.Send(core.InterruptIntent{})
		}
		return true
	case "/steer":
		if rest == "" {
			_ = b.client.sendMessage(ctx, cv.c.ChatID, "(usage: /steer <new instruction>)")
			return true
		}
		if conv == nil {
			msg.text = rest // nothing to steer; run it as the next prompt
			return false
		}
		s.clearAsk(cv)
		s.notePending(cv, rest)
		conv.Send(core.SteerIntent{Text: rest})
		return true
	}
	s.mu.Lock()
	pending := cv.ask != nil
	s.mu.Unlock()
	if !pending {
		return false
	}
	text := s.askReplyText(ctx, b, *msg)
	if text == "" {
		return false // media-only; let it run as a prompt
	}
	if ask := s.takeAsk(cv); ask != nil {
		ask.conv.Send(parseAskReply(ask, text))
		return true
	}
	return false
}

// notePending records a prompt text this bridge injected itself, so the
// mirror tail won't echo it back to the phone that sent it.
func (s *Service) notePending(cv *convo, text string) {
	s.mu.Lock()
	cv.pending = append(cv.pending, text)
	cv.c.LastAt = time.Now().UnixMilli()
	s.mu.Unlock()
}

// recordStreamed notes a segment the streamer already delivered to the phone.
func (s *Service) recordStreamed(cv *convo, text string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	cv.streamed = append(cv.streamed, text)
	if len(cv.streamed) > 32 {
		cv.streamed = cv.streamed[len(cv.streamed)-32:]
	}
}

// consumeStreamed reports whether text was already live-streamed to the
// phone, consuming the record.
func (s *Service) consumeStreamed(cv *convo, text string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, t := range cv.streamed {
		if t == text {
			cv.streamed = append(cv.streamed[:i], cv.streamed[i+1:]...)
			return true
		}
	}
	return false
}

// runOwnTurn runs one turn on a bridge-owned ephemeral actor: acquire was
// already won; start the actor, prompt, drive to the terminal event, tear the
// actor down, release. push live-streams the turn's envelopes to the phone;
// whatever the stream didn't carry, the tail still mirrors. While the turn
// runs the queue stays live: a pending question's answer, /steer, and /stop
// act on the turn immediately; plain messages are returned as backlog for
// the caller to run next, in arrival order.
func (s *Service) runOwnTurn(ctx context.Context, b *botRunner, cv *convo, sessID core.SessionID, intent core.PromptIntent, push func(core.Envelope)) []inbound {
	defer engine.Sessions.Release(sessID)
	// A question that outlived its turn must not capture the next message.
	defer s.clearAsk(cv)
	tools := s.botTools(b)

	actorCtx, stopActor := context.WithCancel(ctx)
	defer stopActor()
	eng := s.engineFor(tools)
	conv, err := eng.StartSession(actorCtx, sessID,
		engine.WithOrigin(fmt.Sprintf("telegram:@%s/%d", b.cfg.Username, cv.c.ChatID)),
		engine.WithPrincipal(core.PrincipalLocal),
	)
	if err != nil {
		s.logf("messenger: start session %s: %v", sessID, err)
		s.consumePending(cv, intent.Text) // never landed; don't let it eat a mirror
		_ = b.client.sendMessage(ctx, cv.c.ChatID, "(arbos could not open this conversation — check the host logs)")
		return nil
	}

	s.setBusy(cv, true)
	defer s.setBusy(cv, false)

	turnCtx, cancel := context.WithTimeout(ctx, turnTimeout)
	defer cancel()

	// Telegram's typing indicator decays after ~5s; refresh it while the
	// turn runs so the sender sees the agent is alive.
	typingDone := make(chan struct{})
	go func() {
		t := time.NewTicker(4 * time.Second)
		defer t.Stop()
		b.client.sendTyping(turnCtx, cv.c.ChatID)
		for {
			select {
			case <-typingDone:
				return
			case <-t.C:
				b.client.sendTyping(turnCtx, cv.c.ChatID)
			}
		}
	}()
	defer close(typingDone)

	conv.Send(intent)

	// The bridge is no longer deaf mid-turn: approvals still auto-follow the
	// tools toggle, but a question (the ask tool) goes to the phone and waits
	// for the owner's reply — observeAsk presents it and remembers the
	// request; the queue drain below routes the answer back.
	emit := func(env core.Envelope) {
		push(env)
		s.observeAsk(ctx, b, cv, conv, sessID, env)
		if env.SessionID != sessID {
			return
		}
		if e, ok := env.Event.(core.ApprovalRequest); ok {
			conv.Send(core.ApprovalResponseIntent{
				RequestID: e.RequestID,
				Approved:  tools,
				Reason:    "telegram bridge auto-response",
			})
		}
	}

	// Drive the turn in the background and keep servicing the chat's queue:
	// an answer to a suspended question, /steer, or /stop must reach the
	// actor NOW — the old blocking drive left the phone locked out of its
	// own turn until timeout. Plain messages backlog for after the turn.
	var tc core.TurnComplete
	driveDone := make(chan struct{})
	go func() {
		defer close(driveDone)
		tc, err = engine.Drive(turnCtx, conv, emit)
	}()
	var backlog []inbound
	for waiting := true; waiting; {
		select {
		case <-driveDone:
			waiting = false
		case <-turnCtx.Done():
			eng.Interrupt(sessID) // stop the still-running turn, not just our wait
			<-driveDone
			waiting = false
		case msg := <-cv.queue:
			if !s.routeTurn(ctx, b, cv, conv, &msg) {
				backlog = append(backlog, msg)
			}
		}
	}

	// handleInbound kicks the tail once the stream has settled.
	switch {
	case err == nil:
		if tc.FinalResponse == "" {
			// Nothing for the tail to mirror; don't leave the phone silent.
			_ = b.client.sendMessage(ctx, cv.c.ChatID, "(done — the turn produced no text reply)")
		}
	case turnCtx.Err() != nil:
		_ = b.client.sendMessage(ctx, cv.c.ChatID, "(the turn timed out and was stopped)")
	case errors.Is(err, context.Canceled):
		_ = b.client.sendMessage(ctx, cv.c.ChatID, "(the turn was interrupted)")
	default:
		s.logf("messenger: turn %s: %v", sessID, err)
		_ = b.client.sendMessage(ctx, cv.c.ChatID, fmt.Sprintf("(turn failed: %v)", err))
	}
	return backlog
}

// buildPrompt reduces one Telegram message to a PromptIntent: text plus any
// media as content parts (photos as vision input, voice transcribed, PDFs as
// document input; anything else degrades to a labeled placeholder).
func (s *Service) buildPrompt(ctx context.Context, b *botRunner, cv *convo, msg inbound) (core.PromptIntent, error) {
	text := msg.text
	var parts []core.ContentBlock

	if msg.photo != "" {
		img, err := b.client.fetchFile(ctx, msg.photo)
		if err != nil {
			return core.PromptIntent{}, fmt.Errorf("photo: %w", err)
		}
		parts = append(parts, core.ContentBlock{
			Type:  core.BlockImage,
			Image: &core.ImageData{Data: base64.StdEncoding.EncodeToString(img), MimeType: "image/jpeg"},
		})
	}

	if msg.voice != nil {
		switch {
		case s.cfg.Transcribe == nil:
			text = strings.TrimSpace("[voice message — no transcriber configured] " + text)
		default:
			audio, err := b.client.fetchFile(ctx, msg.voice.FileID)
			if err != nil {
				return core.PromptIntent{}, fmt.Errorf("voice: %w", err)
			}
			spoken, err := s.cfg.Transcribe(ctx, base64.StdEncoding.EncodeToString(audio), "ogg")
			if err != nil {
				return core.PromptIntent{}, fmt.Errorf("transcribe: %w", err)
			}
			text = strings.TrimSpace(spoken + " " + text)
		}
	}

	if msg.doc != nil {
		if msg.doc.MimeType == "application/pdf" && msg.doc.FileSize <= mediaCap {
			pdf, err := b.client.fetchFile(ctx, msg.doc.FileID)
			if err != nil {
				return core.PromptIntent{}, fmt.Errorf("document: %w", err)
			}
			parts = append(parts, core.ContentBlock{
				Type: core.BlockFile,
				File: &core.FileData{Data: base64.StdEncoding.EncodeToString(pdf), MimeType: "application/pdf", Name: msg.doc.FileName},
			})
		} else {
			text = strings.TrimSpace(fmt.Sprintf("[file attached on Telegram: %s (%s) — not carried over]", msg.doc.FileName, msg.doc.MimeType) + " " + text)
		}
	}

	if text == "" && len(parts) > 0 {
		text = "(see attached)"
	}
	// A slash command must keep its leading slash for the engine's template
	// expander — no sender prefix, no preface; those wait for a plain turn.
	text = s.resolveCommand(text, b.cfg.Username)
	if !strings.HasPrefix(text, "/") {
		if cv.c.Kind != "private" {
			// Group chats interleave speakers; the model needs to know who is talking.
			text = msg.from.name() + ": " + text
		}
		if s.firstPrompt(cv) {
			text = fmt.Sprintf("[Telegram bridge: this conversation reaches you via your Telegram bot @%s, from %s. Replies mirror to their phone as plain-text Telegram messages — keep them concise and skip heavy markdown.]\n\n",
				b.cfg.Username, msg.from.name()) + text
		}
	}
	return core.PromptIntent{Text: text, Parts: parts, Origin: originTelegram}, nil
}

// firstPrompt reports whether this conversation's session has never seen a
// prompt — the one turn that carries the bridge preface.
func (s *Service) firstPrompt(cv *convo) bool {
	s.mu.Lock()
	next := cv.c.NextSeq
	prime := cv.prime
	s.mu.Unlock()
	if next > 0 || prime {
		return false
	}
	evs, err := s.cfg.Store.EventsFrom(context.Background(), core.SessionID(cv.c.SessionID), 0)
	return err == nil && len(evs) == 0
}

func (s *Service) setBusy(cv *convo, busy bool) {
	s.mu.Lock()
	cv.c.Busy = busy
	s.mu.Unlock()
	s.broadcastConvo(cv)
}

// --- the mirror tail -------------------------------------------------------------

// runTail is the outbound half of the duplex: a poll over every bridged
// session's event log, mirroring whatever is new — from whichever door caused
// it — to the Telegram thread. This is the only path agent output takes to
// the phone, so bridge-run turns and web-run turns deliver identically. It
// also keeps the phone's typing indicator honest while a turn is mid-run.
func (s *Service) runTail(ctx context.Context) {
	tick := time.NewTicker(tailInterval)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		case <-s.kick:
		}
		s.mu.Lock()
		convos := make([]*convo, 0, len(s.convos))
		for _, cv := range s.convos {
			convos = append(convos, cv)
		}
		s.mu.Unlock()
		dirty := false
		for _, cv := range convos {
			if s.tailConvo(ctx, cv) {
				dirty = true
			}
			s.refreshTyping(ctx, cv)
		}
		if dirty {
			s.mu.Lock()
			s.saveLocked()
			s.mu.Unlock()
		}
	}
}

// kickTail nudges the tail to run now (a turn just finished; deliver its
// reply immediately instead of a poll later).
func (s *Service) kickTail() {
	select {
	case s.kick <- struct{}{}:
	default:
	}
}

// refreshTyping keeps the Telegram typing indicator alive while a turn is in
// flight on this conversation — including turns the bridge didn't run (an
// open web tab's), which it can only see through the log.
func (s *Service) refreshTyping(ctx context.Context, cv *convo) {
	s.mu.Lock()
	bot := s.bots[cv.c.BotID]
	due := cv.inFlight && time.Since(cv.lastTyping) >= typingRefresh
	if cv.inFlight && time.Since(cv.inFlightAt) > turnTimeout {
		cv.inFlight = false // a stuck flag must not type forever
		due = false
	}
	if due {
		cv.lastTyping = time.Now()
	}
	s.mu.Unlock()
	if due && bot != nil {
		bot.client.sendTyping(ctx, cv.c.ChatID)
	}
}

// runOutbox makes the phone an outbox door — the agent's voice between turns
// (scheduled firings, finished background work, escalations) reaches the
// owner's pocket. Same claim-then-deliver contract as the web and terminal
// doors: a claimed message is this door's alone, so nothing is ever heard
// twice. A bridged session's notice routes to its own chat; broadcast-class
// and stale-swept notices go to the owner — the most recently active private
// chat on a full-permission bot. No such chat = no claim (a claim with no
// deliverable target would silently eat the message).
func (s *Service) runOutbox(ctx context.Context) {
	tick := time.NewTicker(outboxPoll)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}

		s.mu.Lock()
		bySession := map[string]*convo{}
		sessions := make([]string, 0, len(s.convos))
		var owner *convo
		for _, cv := range s.convos {
			bySession[cv.c.SessionID] = cv
			sessions = append(sessions, cv.c.SessionID)
			b := s.bots[cv.c.BotID]
			if b == nil || !b.cfg.Tools || cv.c.Kind != "private" {
				continue
			}
			if owner == nil || cv.c.LastAt > owner.c.LastAt {
				owner = cv
			}
		}
		s.mu.Unlock()
		if owner == nil {
			continue
		}

		msgs, err := s.cfg.Store.ClaimOutboxFor(ctx, viaTelegram, core.PrincipalLocal,
			sessions, time.Now().Add(-outbox.StaleAfter))
		if err != nil || len(msgs) == 0 {
			continue
		}
		for _, m := range msgs {
			target := bySession[m.Session]
			if target == nil {
				target = owner
			}
			s.mu.Lock()
			bot := s.bots[target.c.BotID]
			s.mu.Unlock()
			if bot == nil {
				continue
			}
			if err := bot.client.sendMessage(ctx, target.c.ChatID, "🔔 "+m.Text); err != nil {
				s.logf("messenger: @%s notify: %v", bot.cfg.Username, err)
			}
		}
	}
}

// tailConvo mirrors one conversation's new events to Telegram, returning
// whether the high-water mark moved.
func (s *Service) tailConvo(ctx context.Context, cv *convo) bool {
	s.mu.Lock()
	next := cv.c.NextSeq
	prime := cv.prime
	bot := s.bots[cv.c.BotID]
	s.mu.Unlock()
	if bot == nil {
		return false
	}

	events, err := s.cfg.Store.EventsFrom(ctx, core.SessionID(cv.c.SessionID), next)
	if err != nil {
		s.logf("messenger: tail %s: %v", cv.c.SessionID, err)
		return false
	}
	if len(events) == 0 {
		return false
	}

	if prime {
		// First tail over a pre-existing log: fast-forward, never re-deliver
		// history the phone already lived through.
		s.mu.Lock()
		cv.c.NextSeq = events[len(events)-1].Seq + 1
		cv.prime = false
		s.mu.Unlock()
		return true
	}

	for _, ev := range events {
		s.mirrorEvent(ctx, bot, cv, ev)
		s.mu.Lock()
		cv.c.NextSeq = ev.Seq + 1
		cv.c.LastAt = time.Now().UnixMilli()
		if flight, known := turnInFlight(ev); known {
			cv.inFlight = flight
			cv.inFlightAt = time.Now()
		}
		s.mu.Unlock()
	}
	s.broadcastConvo(cv)
	return true
}

// turnInFlight reads one log event as turn progress: a prompt or tool
// activity means a reply is still coming (keep the phone's typing indicator
// alive); a plain assistant message or an interrupt means the turn settled.
// Events that say nothing about progress (config, usage) return known=false.
func turnInFlight(ev core.Event) (flight, known bool) {
	switch p := ev.Payload.(type) {
	case core.MessagePayload:
		switch p.Message.Role {
		case core.RoleUser:
			return true, true
		case core.RoleAssistant:
			return len(p.Message.ToolCalls) > 0, true
		}
		return false, false
	case core.ToolResultPayload:
		return true, true
	case core.InterruptPayload:
		return false, true
	default:
		return false, false
	}
}

// mirrorEvent delivers one log event to the Telegram thread: assistant text
// and images as the agent speaking; user messages from other doors prefixed
// as the owner's own words crossing over; interrupts as a marker. Tool
// chatter stays out — the phone gets the conversation, not the machinery.
func (s *Service) mirrorEvent(ctx context.Context, bot *botRunner, cv *convo, ev core.Event) {
	p, ok := ev.Payload.(core.MessagePayload)
	if !ok {
		if _, interrupted := ev.Payload.(core.InterruptPayload); interrupted {
			_ = bot.client.sendMessage(ctx, cv.c.ChatID, "(interrupted)")
		}
		return
	}
	m := p.Message
	switch m.Role {
	case core.RoleUser:
		if s.consumePending(cv, m.Content) {
			return // our own injection — the phone already has it
		}
		if m.Content != "" {
			if err := bot.client.sendMessage(ctx, cv.c.ChatID, "[from web] "+m.Content); err != nil {
				s.logf("messenger: @%s mirror: %v", bot.cfg.Username, err)
			}
		}
		s.mirrorImages(ctx, bot, cv, m.Parts)
	case core.RoleAssistant:
		if m.Content != "" && !s.consumeStreamed(cv, m.Content) {
			if err := bot.client.sendMessage(ctx, cv.c.ChatID, m.Content); err != nil {
				s.logf("messenger: @%s send: %v", bot.cfg.Username, err)
			}
		}
		s.mirrorImages(ctx, bot, cv, m.Parts)
	}
}

// mirrorImages sends a message's image parts as photos (attachments from the
// web composer, provider-generated images).
func (s *Service) mirrorImages(ctx context.Context, bot *botRunner, cv *convo, parts []core.ContentBlock) {
	for _, part := range parts {
		if part.Type != core.BlockImage || part.Image == nil {
			continue
		}
		img, err := base64.StdEncoding.DecodeString(part.Image.Data)
		if err != nil || len(img) == 0 || len(img) > mediaCap {
			continue
		}
		if err := bot.client.sendPhoto(ctx, cv.c.ChatID, img); err != nil {
			s.logf("messenger: @%s photo: %v", bot.cfg.Username, err)
		}
	}
}

// consumePending reports whether text matches a prompt this bridge injected
// itself, consuming the entry.
func (s *Service) consumePending(cv *convo, text string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, t := range cv.pending {
		if t == text {
			cv.pending = append(cv.pending[:i], cv.pending[i+1:]...)
			return true
		}
	}
	return false
}
