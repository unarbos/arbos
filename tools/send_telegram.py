#!/usr/bin/env python3
"""Send a message to the operator via Telegram.

Usage:
    python tools/send_telegram.py "Your message here"
    python tools/send_telegram.py --file path/to/file.txt
"""
import argparse
import sys
from pathlib import Path

WORKING_DIR = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description="Send a Telegram message to the operator")
    parser.add_argument("message", nargs="?", help="Message text to send")
    parser.add_argument("--file", help="Send contents of a file instead")
    args = parser.parse_args()

    if not args.message and not args.file:
        parser.error("Provide a message or --file")

    from dotenv import load_dotenv
    import os
    load_dotenv(WORKING_DIR / ".env")

    token = os.getenv("TAU_BOT_TOKEN")
    if not token:
        print("ERROR: TAU_BOT_TOKEN not set in .env", file=sys.stderr)
        sys.exit(1)

    chat_id_file = WORKING_DIR / "chat_id.txt"
    if not chat_id_file.exists():
        print("ERROR: chat_id.txt not found — operator hasn't messaged the bot yet", file=sys.stderr)
        sys.exit(1)

    chat_id = chat_id_file.read_text().strip()

    if args.file:
        text = Path(args.file).read_text()
    else:
        text = args.message

    if len(text) > 4000:
        text = text[:4000] + "\n…(truncated)"

    import telebot
    bot = telebot.TeleBot(token)
    bot.send_message(chat_id, text)
    print(f"Sent ({len(text)} chars)")

    # Log to chatlog
    import json
    from datetime import datetime
    chatlog_dir = WORKING_DIR / "context" / "chat"
    chatlog_dir.mkdir(exist_ok=True)
    existing = sorted(chatlog_dir.glob("*.jsonl"))
    current = None
    if existing and existing[-1].stat().st_size < 4000:
        current = existing[-1]
    if current is None:
        current = chatlog_dir / f"{datetime.now().strftime('%Y%m%d_%H%M%S')}.jsonl"
    entry = json.dumps({"role": "bot", "text": text[:1000], "ts": datetime.now().isoformat()})
    with open(current, "a", encoding="utf-8") as f:
        f.write(entry + "\n")


if __name__ == "__main__":
    main()
