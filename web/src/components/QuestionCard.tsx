import {
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type Ref,
} from "react";
import { ChevronDown, ChevronUp, MessageCircleQuestion } from "lucide-react";

import type { PendingQuestions } from "@/lib/transcript";
import type { Question, QuestionAnswer } from "@/lib/types";

/**
 * The ask tool's pending form, rendered the way Cursor renders AskQuestion:
 * a "Questions" panel above the composer with one page per question —
 * numbered bold prompt, lettered option rows (A/B/C…), an "Other…" row that
 * opens a free-text input — and a footer with Skip (Esc) / Continue (⏎).
 * The composer below doubles as "Add more optional details".
 */

export interface QuestionCardHandle {
  /** Toggle the option behind a letter key on the current page. */
  press(letter: string): boolean;
  /** Continue: submit every page's selections. */
  submit(): void;
  /** Skip the whole form. */
  skip(): void;
}

interface Draft {
  selected: string[];
  other: boolean;
  otherText: string;
}

const emptyDraft: Draft = { selected: [], other: false, otherText: "" };

function toggled(q: Question, draft: Draft, optionId: string): Draft {
  const has = draft.selected.includes(optionId);
  if (q.allow_multiple) {
    return {
      ...draft,
      selected: has
        ? draft.selected.filter((id) => id !== optionId)
        : [...draft.selected, optionId],
    };
  }
  return { ...draft, selected: has ? [] : [optionId], other: false };
}

function letterOf(i: number): string {
  return String.fromCharCode(65 + i);
}

export function QuestionCard({
  request,
  onAnswer,
  handleRef,
}: {
  request: PendingQuestions;
  /** Deliver the user's answers (or the skip) to the seam. */
  onAnswer: (answers: QuestionAnswer[], skipped: boolean) => void;
  /** Keyboard seam: the composer routes Enter / Esc / letter keys here. */
  handleRef: Ref<QuestionCardHandle>;
}) {
  const qs = request.questions;
  const [idx, setIdx] = useState(0);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const otherRef = useRef<HTMLInputElement>(null);

  const q = qs[Math.min(idx, qs.length - 1)];
  const draft = drafts[q.id] ?? emptyDraft;
  const setDraft = (d: Draft) => setDrafts((m) => ({ ...m, [q.id]: d }));

  // Selecting "Other…" opens its input — hand it the keyboard.
  useEffect(() => {
    if (draft.other) otherRef.current?.focus();
  }, [draft.other]);

  const buildAnswers = (): QuestionAnswer[] =>
    qs.map((question) => {
      const d = drafts[question.id] ?? emptyDraft;
      const a: QuestionAnswer = { question_id: question.id };
      if (d.selected.length > 0) a.selected_ids = d.selected;
      if (d.other && d.otherText.trim()) a.other_text = d.otherText.trim();
      return a;
    });

  const submit = () => {
    const answers = buildAnswers();
    const answered = answers.some((a) => a.selected_ids || a.other_text);
    onAnswer(answered ? answers : [], !answered);
  };
  const skip = () => onAnswer([], true);

  useImperativeHandle(handleRef, () => ({
    press(letter: string): boolean {
      const i = letter.toUpperCase().charCodeAt(0) - 65;
      if (i < 0 || i > q.options.length) return false;
      if (i === q.options.length) {
        // The synthetic "Other…" row.
        setDraft({
          ...draft,
          other: !draft.other,
          selected: q.allow_multiple ? draft.selected : [],
        });
        return true;
      }
      setDraft(toggled(q, draft, q.options[i].id));
      return true;
    },
    submit,
    skip,
  }));

  const page = (delta: number) =>
    setIdx((i) => (i + delta + qs.length) % qs.length);

  return (
    <div className="rounded-[10px] border border-line bg-card px-3.5 pb-3 pt-2.5">
      <div className="flex items-center gap-1.5">
        <MessageCircleQuestion size={14} className="shrink-0 text-muted" />
        <span className="text-[12px] text-muted">
          Questions{request.title ? ` · ${request.title}` : ""}
        </span>
        <span className="flex-1" />
        <button
          type="button"
          onClick={() => page(-1)}
          disabled={qs.length < 2}
          className="flex size-5 items-center justify-center rounded text-faint transition-colors enabled:cursor-pointer enabled:hover:bg-hover enabled:hover:text-text"
        >
          <ChevronUp size={13} />
        </button>
        <span className="text-[11.5px] text-faint select-none">
          {idx + 1} of {qs.length}
        </span>
        <button
          type="button"
          onClick={() => page(1)}
          disabled={qs.length < 2}
          className="flex size-5 items-center justify-center rounded text-faint transition-colors enabled:cursor-pointer enabled:hover:bg-hover enabled:hover:text-text"
        >
          <ChevronDown size={13} />
        </button>
      </div>

      <div className="mt-2.5 font-medium text-bright">
        {idx + 1}. {q.prompt}
      </div>

      <div className="mt-2 space-y-1">
        {q.options.map((o, i) => {
          const selected = draft.selected.includes(o.id);
          return (
            <OptionRow
              key={o.id}
              letter={letterOf(i)}
              label={o.label}
              selected={selected}
              onClick={() => setDraft(toggled(q, draft, o.id))}
            />
          );
        })}
        <OptionRow
          letter={letterOf(q.options.length)}
          label="Other…"
          muted
          selected={draft.other}
          onClick={() =>
            setDraft({
              ...draft,
              other: !draft.other,
              selected: q.allow_multiple ? draft.selected : [],
            })
          }
        />
        {draft.other && (
          <input
            ref={otherRef}
            value={draft.otherText}
            onChange={(e) => setDraft({ ...draft, otherText: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submit();
              }
              if (e.key === "Escape") {
                e.preventDefault();
                skip();
              }
            }}
            placeholder="Your answer…"
            data-keep-focus
            className="ml-7 block w-[calc(100%-1.75rem)] rounded-md border border-line bg-panel px-2 py-1 text-[12.5px] text-bright outline-none placeholder:text-faint focus:border-accent/50"
          />
        )}
      </div>

      <div className="mt-3 flex items-center justify-end gap-3">
        <button
          type="button"
          onClick={skip}
          className="cursor-pointer text-[12px] text-muted transition-colors hover:text-text"
        >
          Skip <span className="text-faint">Esc</span>
        </button>
        <button
          type="button"
          onClick={submit}
          className="cursor-pointer rounded-md bg-btn px-3 py-0.5 text-[12px] font-medium text-canvas transition-opacity hover:opacity-90"
        >
          Continue ⏎
        </button>
      </div>
    </div>
  );
}

/** One lettered option: `[A] label`, accent-lit when selected. */
function OptionRow({
  letter,
  label,
  selected,
  muted,
  onClick,
}: {
  letter: string;
  label: string;
  selected: boolean;
  muted?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`-mx-1.5 flex w-[calc(100%+0.75rem)] cursor-pointer items-center gap-2.5 rounded-md px-1.5 py-1 text-left transition-colors ${
        selected ? "bg-accent/15" : "hover:bg-hover"
      }`}
    >
      <span
        className={`flex size-[18px] shrink-0 items-center justify-center rounded border text-[10.5px] font-medium ${
          selected
            ? "border-accent/60 bg-accent/20 text-bright"
            : "border-line bg-panel text-muted"
        }`}
      >
        {letter}
      </span>
      <span
        className={`min-w-0 flex-1 break-words text-[13px] ${
          selected ? "text-bright" : muted ? "text-faint" : "text-text"
        }`}
      >
        {label}
      </span>
    </button>
  );
}
