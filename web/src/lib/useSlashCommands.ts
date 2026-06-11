import { useCallback, useEffect, useMemo, useState } from "react";
import type { KeyboardEvent } from "react";

import { fetchCommands, type SlashCommand } from "@/lib/api";

/**
 * The composer's slash-command menu, as one self-contained interaction:
 * detects when the composer is "typing a command" (its whole content is
 * `/name` — slash then a partial name, no whitespace yet), fetches the host's
 * command list on menu open, filters by prefix, and owns the keyboard
 * protocol (↑/↓ move, Tab/Enter accept, Esc dismisses until the text changes
 * shape). Expansion happens server-side at projection; this is discovery and
 * completion only.
 *
 * The menu is also where commands are made: every row carries an edit pencil,
 * and a typed name matching nothing gets a trailing "create" row — both open
 * the template file in a prompt-editor panel (openPrompt) and clear the
 * composer, since the action moved to the panel.
 */
export function useSlashCommands(
  text: string,
  setText: (s: string) => void,
  focus: () => void,
  openPrompt: (name: string, path?: string) => void,
) {
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [highlight, setHighlight] = useState(0);
  const [dismissed, setDismissed] = useState(false);

  const query = useMemo(() => {
    const m = /^\/(\S*)$/.exec(text);
    return m ? m[1] : null;
  }, [text]);
  const active = query !== null;

  // Re-fetch on each menu open (not per keystroke) so a freshly added prompt
  // file shows up without a reload.
  useEffect(() => {
    if (!active) {
      setDismissed(false);
      return;
    }
    fetchCommands().then(setCommands).catch(() => {});
  }, [active]);

  const matches = useMemo(() => {
    if (query === null) return [];
    const q = query.toLowerCase();
    return commands.filter((c) => c.name.toLowerCase().startsWith(q));
  }, [commands, query]);

  // A typed name matching no command exactly can become one: the menu offers
  // a trailing "create /name" row (valid names only — a name is a filename).
  const createName = useMemo(() => {
    if (!query || !/^[\w-]+$/.test(query)) return undefined;
    const q = query.toLowerCase();
    return matches.some((c) => c.name.toLowerCase() === q) ? undefined : query;
  }, [matches, query]);
  const rowCount = matches.length + (createName ? 1 : 0);

  useEffect(() => setHighlight(0), [query]);
  // Clamp when an async refetch shrinks the list under the highlight —
  // otherwise Enter could fall through the menu and send a partial "/p" raw.
  useEffect(() => {
    setHighlight((h) => Math.min(h, Math.max(0, rowCount - 1)));
  }, [rowCount]);

  const open = active && !dismissed && rowCount > 0;

  // Accept a command: the name lands in the composer with a trailing space so
  // the user types args and sends.
  const pick = useCallback(
    (c: SlashCommand) => {
      setText(`/${c.name} `);
      focus();
    },
    [setText, focus],
  );

  // Open a command's template in the editor panel; the composer empties —
  // the action moved to the panel, there is nothing left to send.
  const pickEdit = useCallback(
    (c: SlashCommand) => {
      openPrompt(c.name, c.path);
      setText("");
    },
    [openPrompt, setText],
  );

  // Create: same panel, but the file doesn't exist yet — the editor opens
  // seeded and saves it into existence.
  const pickCreate = useCallback(
    (name: string) => {
      openPrompt(name);
      setText("");
    },
    [openPrompt, setText],
  );

  /**
   * The menu's slice of the composer's keydown. Returns true when the key was
   * consumed; Enter on a command already typed out in full is NOT consumed,
   * so it falls through to the composer's submit.
   */
  const handleKey = (e: KeyboardEvent): boolean => {
    if (!open) return false;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlight((h) => Math.min(h + 1, rowCount - 1));
      return true;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlight((h) => Math.max(h - 1, 0));
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setDismissed(true);
      return true;
    }
    if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
      // The trailing create row: Enter/Tab on it opens the editor panel.
      if (createName && highlight === matches.length) {
        e.preventDefault();
        pickCreate(createName);
        return true;
      }
      const c = matches[highlight];
      if (c && c.name.toLowerCase() !== query?.toLowerCase()) {
        e.preventDefault();
        pick(c);
        return true;
      }
      if (e.key === "Tab") {
        e.preventDefault();
        return true;
      }
    }
    return false;
  };

  return {
    open,
    matches,
    highlight,
    setHighlight,
    pick,
    pickEdit,
    createName,
    pickCreate,
    handleKey,
  };
}
