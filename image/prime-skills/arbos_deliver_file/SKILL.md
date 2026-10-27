---
name: arbos_deliver_file
description: Hand a file you produced (pptx, docx, xlsx, pdf, png, mp4, …) to the user as a download card in the chat; generate an image from a prompt and deliver it; post a native chart from structured series.
---

Write the file under `out/` in your project folder (`/work/<project-id>/out/`; relative paths resolve there) (python-pptx, python-docx, openpyxl, Pillow and reportlab are in this kernel; `ffmpeg` is on PATH), then:

```python
await arbos_deliver_file.run("out/q3-deck.pptx", title="Q3 deck")
await arbos_deliver_file.generate_image("a lighthouse at dusk, watercolor", size="1024x1024")
```

The card appears on this subagent's message with the file name, size and a Download button; images show inline. Afterwards mention the file in one line in your summary (and list it under `artifacts` as `kind: "file"`); never paste its contents.

A chart is data, not a picture: `await arbos_deliver_file.post_chart({"kind": "line", "title": "Eval score over runs", "series": [{"name": "Affine 2", "points": [[1, 66.2], [2, 70.4]]}], "x": {"label": "Run"}, "y": {"unit": "%"}}, "Affine 2 passed the baseline at run 9.")` draws a native, interactive chart in the chat (bar/pie take `categories` + `values`). Never plot with matplotlib and deliver the PNG when `post_chart` can carry the data.
