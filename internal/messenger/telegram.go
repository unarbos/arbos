// Telegram Bot API client — the minimal HTTPS surface the bridge needs:
// validate a token (getMe), long-poll for inbound messages (getUpdates), show
// liveness while a turn runs (sendChatAction), and deliver replies
// (sendMessage). Long polling is deliberate: it works from behind any NAT
// with no public URL, which is where most arbos hosts live; webhooks would
// demand the opposite.
package messenger

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

// pollTimeout is the long-poll window passed to getUpdates. Telegram holds
// the request open until a message arrives or the window lapses.
const pollTimeout = 50 * time.Second

// sendLimit is Telegram's hard cap on one message's text. Longer replies are
// split at the nearest line break under the cap.
const sendLimit = 4000

type tgClient struct {
	token string
	http  *http.Client
}

func newTGClient(token string) *tgClient {
	// The client must outwait the long poll, with headroom for the response
	// body; everything else (getMe, sendMessage) finishes far sooner.
	return &tgClient{token: token, http: &http.Client{Timeout: pollTimeout + 10*time.Second}}
}

// tgUser is a Telegram user or bot identity (getMe, message senders).
type tgUser struct {
	ID        int64  `json:"id"`
	FirstName string `json:"first_name"`
	LastName  string `json:"last_name"`
	Username  string `json:"username"`
}

// name renders the friendliest available handle for a user.
func (u tgUser) name() string {
	if u.Username != "" {
		return "@" + u.Username
	}
	if u.LastName != "" {
		return u.FirstName + " " + u.LastName
	}
	return u.FirstName
}

// tgChat is the conversation a message belongs to.
type tgChat struct {
	ID        int64  `json:"id"`
	Type      string `json:"type"` // private | group | supergroup | channel
	Title     string `json:"title"`
	FirstName string `json:"first_name"`
	LastName  string `json:"last_name"`
	Username  string `json:"username"`
}

// title renders the chat's display name (group title, or the peer's name for
// a private chat).
func (c tgChat) title() string {
	if c.Title != "" {
		return c.Title
	}
	return tgUser{FirstName: c.FirstName, LastName: c.LastName, Username: c.Username}.name()
}

// tgPhotoSize is one resolution of an inbound photo; Telegram lists several,
// smallest first.
type tgPhotoSize struct {
	FileID   string `json:"file_id"`
	FileSize int64  `json:"file_size"`
}

// tgVoice is an inbound voice memo (OGG/Opus).
type tgVoice struct {
	FileID   string `json:"file_id"`
	MimeType string `json:"mime_type"`
	FileSize int64  `json:"file_size"`
}

// tgDocument is an inbound file attachment.
type tgDocument struct {
	FileID   string `json:"file_id"`
	FileName string `json:"file_name"`
	MimeType string `json:"mime_type"`
	FileSize int64  `json:"file_size"`
}

// tgMessage is one inbound message; only the fields the bridge reads.
type tgMessage struct {
	MessageID int64         `json:"message_id"`
	From      *tgUser       `json:"from"`
	Chat      tgChat        `json:"chat"`
	Date      int64         `json:"date"`
	Text      string        `json:"text"`
	Caption   string        `json:"caption"`
	Photo     []tgPhotoSize `json:"photo"`
	Voice     *tgVoice      `json:"voice"`
	Document  *tgDocument   `json:"document"`
	Sticker   *struct{}     `json:"sticker"`
}

// tgCallbackQuery is an inline-keyboard button tap: Data is the button's
// callback_data, Message the message the keyboard was attached to (so the
// bridge knows which chat to act on).
type tgCallbackQuery struct {
	ID      string     `json:"id"`
	From    *tgUser    `json:"from"`
	Message *tgMessage `json:"message"`
	Data    string     `json:"data"`
}

type tgUpdate struct {
	UpdateID      int64            `json:"update_id"`
	Message       *tgMessage       `json:"message"`
	CallbackQuery *tgCallbackQuery `json:"callback_query"`
}

// tgInlineButton is one inline-keyboard button. A tap sends CallbackData back
// as a callback_query update.
type tgInlineButton struct {
	Text         string `json:"text"`
	CallbackData string `json:"callback_data"`
}

// inlineKeyboard wraps rows of buttons in the reply_markup shape the Bot API
// expects.
type inlineKeyboard struct {
	InlineKeyboard [][]tgInlineButton `json:"inline_keyboard"`
}

// call performs one Bot API method and decodes its result payload.
func (c *tgClient) call(ctx context.Context, method string, params any, result any) error {
	var body bytes.Buffer
	if params != nil {
		if err := json.NewEncoder(&body).Encode(params); err != nil {
			return err
		}
	}
	url := "https://api.telegram.org/bot" + c.token + "/" + method
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, &body)
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	var envelope struct {
		OK          bool            `json:"ok"`
		Description string          `json:"description"`
		Result      json.RawMessage `json:"result"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&envelope); err != nil {
		return fmt.Errorf("telegram %s: %w", method, err)
	}
	if !envelope.OK {
		return fmt.Errorf("telegram %s: %s", method, envelope.Description)
	}
	if result != nil {
		return json.Unmarshal(envelope.Result, result)
	}
	return nil
}

// getMe validates the token and returns the bot's own identity.
func (c *tgClient) getMe(ctx context.Context) (tgUser, error) {
	var u tgUser
	err := c.call(ctx, "getMe", nil, &u)
	return u, err
}

// getUpdates long-polls for inbound messages after offset.
func (c *tgClient) getUpdates(ctx context.Context, offset int64) ([]tgUpdate, error) {
	params := map[string]any{
		"offset":          offset,
		"timeout":         int(pollTimeout.Seconds()),
		"allowed_updates": []string{"message", "callback_query"},
	}
	var ups []tgUpdate
	err := c.call(ctx, "getUpdates", params, &ups)
	return ups, err
}

// parseMode selects Telegram's rich-text rendering for a message. The empty
// string sends plain text (no markup parsing). The bridge uses "HTML" for
// model prose (see format.go); a parse failure never blocks delivery — the
// send/edit helpers retry the same text plain.
const (
	parseNone = ""
	parseHTML = "HTML"
)

// applyFormat adds parse_mode and suppresses the link preview for a formatted
// message. Mid-conversation link cards are noise; the rich text is the point.
func applyFormat(params map[string]any, parseMode string) {
	if parseMode == parseNone {
		return
	}
	params["parse_mode"] = parseMode
	params["link_preview_options"] = map[string]any{"is_disabled": true}
}

// recoverableFormatError reports whether Telegram rejected a message because
// of its formatting — the failures the bridge recovers from by resending as
// plain text. It covers unparseable entities and the formatting-induced 400s
// that don't mention entities (a bad link scheme, an empty href). Delivery
// must never hinge on formatting, so any of these triggers the plain retry.
func recoverableFormatError(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	for _, s := range []string{"can't parse entities", "unsupported URL protocol", "URL host is empty"} {
		if strings.Contains(msg, s) {
			return true
		}
	}
	return false
}

// callFormatted runs a send/edit with parse_mode set, then — if Telegram
// rejects the formatting — retries with markup stripped, substituting the
// readable plain text (the Markdown source) so the fallback shows clean prose
// rather than literal HTML tags. plain == "" reuses params["text"]. Formatting
// must never cost delivery; other errors pass through unchanged.
func (c *tgClient) callFormatted(ctx context.Context, method string, params map[string]any, parseMode, plain string, result any) error {
	applyFormat(params, parseMode)
	err := c.call(ctx, method, params, result)
	if parseMode != parseNone && recoverableFormatError(err) {
		if plain != "" {
			params["text"] = plain
		}
		delete(params, "parse_mode")
		delete(params, "link_preview_options")
		err = c.call(ctx, method, params, result)
	}
	return err
}

// sendMessage delivers text to a chat, split into chunks under Telegram's
// length cap. With parseMode set the text is formatted; a formatting rejection
// falls back to plain so a stray entity never drops the whole message.
//
// The split is by bytes, so callers passing already-rendered HTML must keep
// the text short enough to not be chunked (a cut would sever a tag); for
// arbitrary-length Markdown use sendMarkdown, which splits the source.
func (c *tgClient) sendMessage(ctx context.Context, chatID int64, text, parseMode string) error {
	for _, chunk := range splitMessage(text, sendLimit) {
		if _, err := c.sendMessageID(ctx, chatID, chunk, parseMode, ""); err != nil {
			return err
		}
	}
	return nil
}

// sendMarkdown delivers arbitrary-length Markdown as Telegram HTML. The split
// happens on the SOURCE before conversion, so a chunk boundary never cuts an
// HTML tag — each chunk is converted and self-balanced on its own (see
// format.go). The source chunk doubles as the plain fallback, so a rejected
// chunk degrades to readable Markdown. Use this, not sendMessage, for text
// whose length isn't bounded.
func (c *tgClient) sendMarkdown(ctx context.Context, chatID int64, src string) error {
	for _, chunk := range splitMessage(src, sendLimit) {
		if _, err := c.sendMessageID(ctx, chatID, mdToHTML(chunk), parseHTML, chunk); err != nil {
			return err
		}
	}
	return nil
}

// sendMessageID sends one message (no chunking) and returns its id — the
// handle live-streaming edits against. On a formatting rejection it resends
// plain (substituting plain, the readable source), so formatting never costs
// delivery.
func (c *tgClient) sendMessageID(ctx context.Context, chatID int64, text, parseMode, plain string) (int64, error) {
	var m struct {
		MessageID int64 `json:"message_id"`
	}
	err := c.callFormatted(ctx, "sendMessage", map[string]any{"chat_id": chatID, "text": text}, parseMode, plain, &m)
	return m.MessageID, err
}

// sendMessageMarkup sends one message carrying an inline keyboard and returns
// its id, so a later edit can strip the buttons once the question is answered.
func (c *tgClient) sendMessageMarkup(ctx context.Context, chatID int64, text string, kb inlineKeyboard, parseMode, plain string) (int64, error) {
	var m struct {
		MessageID int64 `json:"message_id"`
	}
	err := c.callFormatted(ctx, "sendMessage", map[string]any{"chat_id": chatID, "text": text, "reply_markup": kb}, parseMode, plain, &m)
	return m.MessageID, err
}

// answerCallbackQuery acknowledges a button tap so the client clears its
// loading spinner. Best-effort; a failed ack only leaves the spinner briefly.
func (c *tgClient) answerCallbackQuery(ctx context.Context, id, text string) {
	_ = c.call(ctx, "answerCallbackQuery", map[string]any{
		"callback_query_id": id, "text": text,
	}, nil)
}

// editMessageReplyMarkup removes (or replaces) a message's inline keyboard —
// used to strip the buttons once a choice is made so the form can't be
// answered twice. Passing an empty keyboard clears it.
func (c *tgClient) clearReplyMarkup(ctx context.Context, chatID, messageID int64) error {
	return c.call(ctx, "editMessageReplyMarkup", map[string]any{
		"chat_id": chatID, "message_id": messageID,
	}, nil)
}

// editMessageText replaces a sent message's text — how a reply "streams" on
// Telegram: send early, grow in place. On a formatting rejection it retries
// the same text plain, so a mid-stream entity glitch never stalls the edit.
func (c *tgClient) editMessageText(ctx context.Context, chatID, messageID int64, text, parseMode, plain string) error {
	return c.callFormatted(ctx, "editMessageText", map[string]any{"chat_id": chatID, "message_id": messageID, "text": text}, parseMode, plain, nil)
}

// deleteMessage removes a sent message — how the ephemeral activity ticker
// leaves no residue once prose resumes.
func (c *tgClient) deleteMessage(ctx context.Context, chatID, messageID int64) error {
	return c.call(ctx, "deleteMessage", map[string]any{
		"chat_id": chatID, "message_id": messageID,
	}, nil)
}

// mediaCap bounds an inbound or outbound media transfer. Above it the bridge
// degrades to a text placeholder rather than ferrying tens of megabytes
// through the prompt.
const mediaCap = 16 << 20

// getFile resolves a file_id to a downloadable path.
func (c *tgClient) getFile(ctx context.Context, fileID string) (string, error) {
	var f struct {
		FilePath string `json:"file_path"`
	}
	if err := c.call(ctx, "getFile", map[string]any{"file_id": fileID}, &f); err != nil {
		return "", err
	}
	if f.FilePath == "" {
		return "", fmt.Errorf("telegram getFile: empty file_path")
	}
	return f.FilePath, nil
}

// download fetches a file's bytes by the path getFile returned.
func (c *tgClient) download(ctx context.Context, filePath string) ([]byte, error) {
	url := "https://api.telegram.org/file/bot" + c.token + "/" + filePath
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("telegram download: %s", resp.Status)
	}
	return io.ReadAll(io.LimitReader(resp.Body, mediaCap+1))
}

// fetchFile resolves and downloads one file, enforcing the media cap.
func (c *tgClient) fetchFile(ctx context.Context, fileID string) ([]byte, error) {
	path, err := c.getFile(ctx, fileID)
	if err != nil {
		return nil, err
	}
	b, err := c.download(ctx, path)
	if err != nil {
		return nil, err
	}
	if len(b) > mediaCap {
		return nil, fmt.Errorf("telegram file exceeds %d bytes", mediaCap)
	}
	return b, nil
}

// sendPhoto delivers image bytes to a chat (multipart upload).
func (c *tgClient) sendPhoto(ctx context.Context, chatID int64, img []byte) error {
	var body bytes.Buffer
	mw := multipart.NewWriter(&body)
	if err := mw.WriteField("chat_id", strconv.FormatInt(chatID, 10)); err != nil {
		return err
	}
	fw, err := mw.CreateFormFile("photo", "image.png")
	if err != nil {
		return err
	}
	if _, err := fw.Write(img); err != nil {
		return err
	}
	if err := mw.Close(); err != nil {
		return err
	}
	url := "https://api.telegram.org/bot" + c.token + "/sendPhoto"
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, &body)
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	var envelope struct {
		OK          bool   `json:"ok"`
		Description string `json:"description"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&envelope); err != nil {
		return fmt.Errorf("telegram sendPhoto: %w", err)
	}
	if !envelope.OK {
		return fmt.Errorf("telegram sendPhoto: %s", envelope.Description)
	}
	return nil
}

// tgBotCommand is one entry in Telegram's "/" command menu.
type tgBotCommand struct {
	Command     string `json:"command"`
	Description string `json:"description"`
}

// setMyCommands publishes the bot's command menu — what the Telegram client
// offers when the user types "/".
func (c *tgClient) setMyCommands(ctx context.Context, cmds []tgBotCommand) error {
	return c.call(ctx, "setMyCommands", map[string]any{"commands": cmds}, nil)
}

// sendTyping shows the "typing…" indicator; Telegram clears it after ~5s, so
// the caller refreshes it while a turn runs. Best-effort by design.
func (c *tgClient) sendTyping(ctx context.Context, chatID int64) {
	_ = c.call(ctx, "sendChatAction", map[string]any{"chat_id": chatID, "action": "typing"}, nil)
}

// splitMessage breaks text into chunks of at most limit bytes, preferring
// line breaks so chunks read naturally and never splitting mid-rune.
func splitMessage(text string, limit int) []string {
	if text == "" {
		return nil
	}
	var out []string
	for len(text) > limit {
		cut := limit
		for cut > 0 && !utf8.RuneStart(text[cut]) {
			cut--
		}
		if i := strings.LastIndexByte(text[:cut], '\n'); i > limit/2 {
			cut = i + 1
		}
		out = append(out, text[:cut])
		text = text[cut:]
	}
	return append(out, text)
}
