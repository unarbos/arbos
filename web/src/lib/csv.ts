/**
 * Delimited-text (CSV/TSV) parse and serialize for the sheet surface. A sheet
 * is just a file like every other surface — the grid reads it back through the
 * gateway, edits cells, and writes the serialized text out the same door the
 * text editor uses. RFC 4180 rules: fields may be double-quoted, quotes inside
 * a quoted field are doubled, and a quoted field may span newlines.
 */

/** Comma for .csv (and anything else); tab for .tsv/.tab. */
export function delimiterFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return ext === "tsv" || ext === "tab" ? "\t" : ",";
}

/**
 * Parse delimited text into a grid of rows. A trailing newline does not add a
 * blank row; empty input is an empty grid.
 */
export function parseCsv(text: string, delim = ","): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  let started = false; // distinguishes a real empty field from "no data yet"

  const pushField = () => {
    row.push(field);
    field = "";
    started = false;
  };
  const pushRow = () => {
    pushField();
    rows.push(row);
    row = [];
  };

  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          quoted = false;
        }
      } else {
        field += c;
      }
      continue;
    }
    if (c === '"' && field === "") {
      quoted = true;
      started = true;
      continue;
    }
    if (c === delim) {
      pushField();
      started = true; // a delimiter implies a field follows, even if empty
      continue;
    }
    if (c === "\r") continue;
    if (c === "\n") {
      pushRow();
      continue;
    }
    field += c;
    started = true;
  }
  // Flush the final record unless the text ended cleanly on a newline.
  if (started || field !== "" || row.length > 0) pushRow();
  return rows;
}

/** A single cell, quoted only when it contains the delimiter, a quote, or a
 * newline (so a plain value round-trips byte-for-byte). */
function encodeCell(value: string, delim: string): string {
  if (
    value.includes('"') ||
    value.includes(delim) ||
    value.includes("\n") ||
    value.includes("\r")
  ) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

/** Serialize a grid back to delimited text with a trailing newline. */
export function toCsv(rows: string[][], delim = ","): string {
  if (rows.length === 0) return "";
  return (
    rows.map((row) => row.map((cell) => encodeCell(cell, delim)).join(delim)).join("\n") +
    "\n"
  );
}

/** Widest row's column count — the grid renders every row to this width. */
export function columnCount(rows: string[][]): number {
  return rows.reduce((max, row) => Math.max(max, row.length), 0);
}
