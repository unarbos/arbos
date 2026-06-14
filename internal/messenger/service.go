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

// turnTimeout bounds how long a pending phone question stays answerable: an ask
// older than this is stale (its turn is long over) and a later reply is
// conversation again, not its answer.
const turnTimeout = 15 * time.Minute

// convoQueueDepth bounds messages waiting on a chat's worker while a turn
// runs; beyond it the bridge tells the sender it is overloaded.
const convoQueueDepth = 16

// actorAttachRetryMax caps the backoff while a door waits out a cross-engine
// conflict (the other engine's actor winding down through its grace window).
const actorAttachRetryMax = 10 * time.Second

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
	ClaimOutboxFor(ctx context.Context, via, principal string, sessions []string) ([]outbox.Message, error)
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
	// Full is the host's own engine; tools-on bots attach their chat's door
	// here, sharing the one session actor with an open web tab on the same
	// session.
	Full *engine.Engine
	// Guest builds the chat-only engine for tools-off bots, once, on first
	// need (piwire.Host.GuestEngine with the bridge's prompt).
	Guest func(systemPrompt string) *engine.Engine
	// Store is the durable event log: the outbox claim door reads it, and
	// firstPrompt checks it for the bridge preface. Required.
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

// Service owns the bot runners, the per-chat door workers, the outbox door,
// and the panel fan-out.
type Service struct {
	cfg Config
	ctx context.Context // service lifetime, set by Start

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
	// origin tags prompts this Telegram door sends into the shared actor, so
	// the presenter recognizes their Queued echo (already on the phone) and
	// skips it, while messages from other doors (a web tab) cross over. It is
	// this door's identity in the one-session-many-doors model.
	origin string
	// conv is the live session actor this door is attached to (set by
	// runConvo). Inbound messages and control intents Send to it. A callback
	// tap (ask answer) from the poll goroutine reads it too. Guarded by
	// Service.mu.
	conv *engine.Conversation
	// ask is the QuestionRequest a suspended turn is waiting on, presented on
	// the phone and awaiting the owner's next reply (see ask.go). Guarded by
	// Service.mu.
	ask *pendingAsk
	// cancel stops this chat's door worker; SetTools relaunches the worker
	// through it so a tools toggle re-attaches on the engine it now selects.
	// done closes when the worker has fully exited, so a relaunch never races
	// a duplicate. Both guarded by Service.mu.
	cancel context.CancelFunc
	done   chan struct{}
}

// originFor is a Telegram door's stable identity: which bot, which chat. The
// presenter skips a Queued echo carrying it (the phone sent that message), and
// web doors render it as "via telegram".
func originFor(botID, chatID int64) string {
	return fmt.Sprintf("telegram:%d:%d", botID, chatID)
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
		bots:   map[int64]*botRunner{},
		convos: map[string]*convo{},
		subs:   map[chan Event]bool{},
	}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

// Start launches a poller per registered bot, a door worker per known chat,
// and the outbox door. Blocks nothing; everything lives until ctx is cancelled.
func (s *Service) Start(ctx context.Context) {
	s.mu.Lock()
	s.ctx = ctx
	for _, b := range s.bots {
		s.startBotLocked(b)
	}
	s.mu.Unlock()
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

// SetTools flips a bot's permission switch. Because a door attaches to its
// engine once (tools-on → the full host engine, off → the chat-only guest
// engine), the switch only takes effect when the door re-attaches — so this
// relaunches the bot's chat workers, dropping a downgraded bot off the full
// toolset rather than leaving it there until restart. Each relaunched worker's
// retry loop rides out the grace window while the previous engine's actor winds
// down.
func (s *Service) SetTools(id int64, tools bool) (BotView, error) {
	s.mu.Lock()
	b := s.bots[id]
	if b == nil {
		s.mu.Unlock()
		return BotView{}, fmt.Errorf("no bot %d", id)
	}
	b.cfg.Tools = tools
	s.saveLocked()
	s.broadcastStateLocked()
	parent := b.ctx
	view := b.cfg.view()
	workers := make([]*convo, 0)
	for _, cv := range s.convos {
		if cv.c.BotID == id {
			workers = append(workers, cv)
		}
	}
	s.mu.Unlock()

	for _, cv := range workers {
		s.mu.Lock()
		cancel, done := cv.cancel, cv.done
		s.mu.Unlock()
		if cancel != nil {
			cancel()
		}
		if done != nil {
			<-done // wait for a clean exit so the relaunch never doubles up
		}
		s.launchConvo(parent, b, cv)
	}
	return view, nil
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
		s.convos[c.ID] = &convo{
			c:      c,
			queue:  make(chan inbound, convoQueueDepth),
			origin: originFor(c.BotID, c.ChatID),
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
			s.launchConvo(b.ctx, b, cv)
		}
	}
	go s.runBot(b.ctx, b)
	go s.publishCommands(b.ctx, b)
}

// launchConvo starts (or restarts) a chat's door worker under its own
// cancellable context, recording the canceller and a done signal so SetTools
// can stop the worker and wait for a clean exit before relaunching it.
func (s *Service) launchConvo(parent context.Context, b *botRunner, cv *convo) {
	cctx, cancel := context.WithCancel(parent)
	done := make(chan struct{})
	s.mu.Lock()
	cv.cancel = cancel
	cv.done = done
	s.mu.Unlock()
	go func() {
		defer close(done)
		s.runConvo(cctx, b, cv)
	}()
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
			if u.CallbackQuery != nil {
				s.onCallback(ctx, b, *u.CallbackQuery)
				continue
			}
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
			queue:  make(chan inbound, convoQueueDepth),
			origin: originFor(b.cfg.ID, m.Chat.ID),
		}
		s.convos[key] = cv
		s.saveLocked()
		s.broadcastStateLocked()
		s.launchConvo(b.ctx, b, cv)
	}
	s.mu.Unlock()

	select {
	case cv.queue <- in:
	default:
		_ = b.client.sendMessage(ctx, m.Chat.ID, "(arbos is overloaded — message dropped, try again shortly)")
	}
}

// --- inbound turns --------------------------------------------------------------

// runConvo is a chat's standing door onto its session. It attaches to the
// shared session actor — keeping it warm so a turn from ANY door (this phone, a
// web tab) mirrors here live — renders the session's one stream to the phone
// through a presenter, and feeds inbound Telegram messages in as intents. The
// actor serializes turns across every door; this worker just serializes its own
// chat's sends. There is no mirror tail and no dedup: the stream is the single
// source, and the presenter skips this door's own echoes by origin.
func (s *Service) runConvo(ctx context.Context, b *botRunner, cv *convo) {
	sessID := core.SessionID(cv.c.SessionID)
	conv, release := s.attachConvo(ctx, b, cv, sessID)
	if conv == nil {
		return // ctx ended before we could attach
	}

	sub, cancelSub := conv.Subscribe()
	pres := newPresenter(ctx, b.client, cv.c.ChatID, cv.origin, s.logf)
	var present sync.WaitGroup
	present.Add(1)
	go func() {
		defer present.Done()
		for env := range sub {
			s.observeAsk(ctx, b, cv, conv, sessID, env)
			pres.feed(env)
		}
		pres.close()
	}()
	// Order matters: cancelSub closes the stream so the presenter goroutine
	// ends, present.Wait then blocks until it has, and release frees the hub
	// ref last — so a SetTools relaunch sees a fully torn-down worker.
	defer release()
	defer present.Wait()
	defer cancelSub()

	for {
		select {
		case <-ctx.Done():
			return
		case msg := <-cv.queue:
			s.handleInbound(ctx, b, cv, conv, msg)
		}
	}
}

// attachConvo attaches this chat's door to its session, retrying through a
// transient cross-engine conflict instead of killing the worker — a
// full-engine web tab holding the session while a tools-off bot attaches, or
// the grace window after a tools toggle, both clear on their own. Returns nil
// if ctx ends first.
func (s *Service) attachConvo(ctx context.Context, b *botRunner, cv *convo, sessID core.SessionID) (*engine.Conversation, func()) {
	for attempt := 0; ; attempt++ {
		conv, release, err := s.engineFor(s.botTools(b)).Attach(sessID,
			engine.WithOrigin(cv.origin),
			engine.WithPrincipal(core.PrincipalLocal),
		)
		if err == nil {
			s.mu.Lock()
			cv.conv = conv
			s.mu.Unlock()
			return conv, release
		}
		s.logf("messenger: @%s attach %s: %v (retrying)", b.cfg.Username, sessID, err)
		wait := time.Duration(attempt+1) * 2 * time.Second
		if wait > actorAttachRetryMax {
			wait = actorAttachRetryMax
		}
		select {
		case <-ctx.Done():
			return nil, nil
		case <-time.After(wait):
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

// handleInbound feeds one Telegram message into the shared session. Control
// messages (/stop, /steer, a pending question's answer) act on the running
// turn; anything else becomes a prompt the actor runs. The presenter mirrors
// the turn — and any web-tab turn — back to the phone, so there is no separate
// delivery path and nothing to dedup.
func (s *Service) handleInbound(ctx context.Context, b *botRunner, cv *convo, conv *engine.Conversation, msg inbound) {
	if s.routeTurn(ctx, b, cv, conv, &msg) {
		return
	}
	intent, err := s.buildPrompt(ctx, b, cv, msg)
	if err != nil {
		s.logf("messenger: @%s prompt: %v", b.cfg.Username, err)
		_ = b.client.sendMessage(ctx, cv.c.ChatID, "(could not read that message: "+err.Error()+")")
		return
	}
	s.mu.Lock()
	cv.c.LastAt = msg.at.UnixMilli()
	s.mu.Unlock()
	s.broadcastConvo(cv)
	// SendCtx (not Send) so a worker relaunch — SetTools cancelling this ctx —
	// can't wedge on a momentarily full intent buffer.
	conv.SendCtx(ctx, intent)
}

// routeTurn intercepts the messages that act on the running turn rather than
// becoming prompts: /stop interrupts it, /steer replaces it (the engine cancels
// the current turn silently and runs the new text), and a reply while a
// question is pending answers it — so a turn suspended on the ask tool can
// always be unblocked, redirected, or killed from the phone. These built-ins
// shadow any prompt templates of the same names. conv is the session's actor.
// Returns true when the message was consumed; false leaves it to the prompt
// path. A /stop or /steer with nothing running is harmless (the actor is idle).
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
		// Urgent: never let a full intent buffer drop the interrupt.
		if !conv.TrySend(core.InterruptIntent{}) {
			go conv.Send(core.InterruptIntent{})
		}
		return true
	case "/steer":
		if rest == "" {
			_ = b.client.sendMessage(ctx, cv.c.ChatID, "(usage: /steer <new instruction>)")
			return true
		}
		s.clearAsk(cv)
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
		if s.cfg.Transcribe == nil {
			text = strings.TrimSpace("[voice message — no transcriber configured] " + text)
		} else {
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
	// Origin is this door's identity: the presenter skips its own echo, web
	// doors render it as "via telegram".
	return core.PromptIntent{Text: text, Parts: parts, Origin: cv.origin}, nil
}

// firstPrompt reports whether this conversation's session has never seen a
// prompt — the one turn that carries the bridge preface. It reads the log
// directly: an empty log means a fresh session.
func (s *Service) firstPrompt(cv *convo) bool {
	evs, err := s.cfg.Store.EventsFrom(context.Background(), core.SessionID(cv.c.SessionID), 0)
	return err == nil && len(evs) == 0
}

// --- the outbox door -------------------------------------------------------------

// runOutbox makes the phone an outbox door — the agent's voice between turns
// (scheduled firings, finished background work, escalations) reaches the
// owner's pocket. Same claim-then-deliver contract as the web and terminal
// doors: a claimed message is this door's alone, so nothing is ever heard
// twice. Delivery is strictly session-scoped: a bridged session's notice
// routes to its own chat, and broadcast-class notices (which have no
// conversation of their own) go to the owner — the most recently active
// private chat on a full-permission bot. A notice from a session this bridge
// holds no chat for is never claimed here; it waits, durably, for its own
// conversation to reopen rather than spilling into an unrelated chat (e.g. a
// throwaway `arbos -once` automation run sharing this host's store). No owner
// = no claim (a claim with no deliverable target would silently eat the
// message).
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

		msgs, err := s.cfg.Store.ClaimOutboxFor(ctx, viaTelegram, core.PrincipalLocal, sessions)
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

