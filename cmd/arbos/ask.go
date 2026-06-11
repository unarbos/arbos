package main

import (
	"fmt"
	"strings"

	"github.com/unarbos/arbos/internal/core"
)

// This file is the terminal half of the ask tool (core.QuestionRequest): the
// question block both terminal frontends print, and the line parser that turns
// what the user typed into a core.QuestionAnswer. The web frontend renders the
// same request as its Questions panel; here it is lettered options on stdout
// and one answer line per question.

// questionLines renders one question as printable lines: the form header (on
// the first question), the prompt, its lettered options, and the answer hint.
func questionLines(req core.QuestionRequest, idx int) []string {
	q := req.Questions[idx]
	var lines []string
	if idx == 0 {
		header := "Questions"
		if req.Title != "" {
			header += " — " + req.Title
		}
		lines = append(lines, header)
	}
	prompt := q.Prompt
	if len(req.Questions) > 1 {
		prompt = fmt.Sprintf("%d/%d %s", idx+1, len(req.Questions), q.Prompt)
	}
	lines = append(lines, prompt)
	for i, o := range q.Options {
		lines = append(lines, fmt.Sprintf("  %c. %s", 'A'+i, o.Label))
	}
	hint := "answer with a letter, your own words, or enter to skip"
	if q.AllowMultiple {
		hint = "answer with letters (\"a c\"), your own words, or enter to skip"
	}
	lines = append(lines, hint)
	return lines
}

// parseQuestionAnswer maps a typed line onto one question: option letters
// select, anything else is the user's own words, an empty line answers
// nothing.
func parseQuestionAnswer(q core.Question, line string) core.QuestionAnswer {
	ans := core.QuestionAnswer{QuestionID: q.ID}
	s := strings.TrimSpace(line)
	if s == "" {
		return ans
	}
	fields := strings.FieldsFunc(strings.ToLower(s), func(r rune) bool {
		return r == ' ' || r == ','
	})
	var sel []string
	for _, f := range fields {
		if len(f) == 1 && f[0] >= 'a' && int(f[0]-'a') < len(q.Options) {
			sel = append(sel, q.Options[f[0]-'a'].ID)
			continue
		}
		sel = nil
		break
	}
	switch {
	case len(sel) == 0:
		ans.OtherText = s
	case q.AllowMultiple:
		ans.SelectedIDs = sel
	default:
		ans.SelectedIDs = sel[:1]
	}
	return ans
}

// questionsSkipped reports whether every question went unanswered — the
// whole-form skip the web panel sends explicitly.
func questionsSkipped(answers []core.QuestionAnswer) bool {
	for _, a := range answers {
		if len(a.SelectedIDs) > 0 || a.OtherText != "" {
			return false
		}
	}
	return true
}
