import type { ContentBlock } from "@/lib/types";

/**
 * The image content blocks of a prompt as data-URL thumbnails — the single
 * home for part→<img> rendering (user cards, queued-message chips). Renders
 * nothing when there are no images; `wrap` adds a container div around them.
 */
export function PartImages({
  parts,
  className,
  wrap,
}: {
  parts: ContentBlock[] | undefined;
  className: string;
  wrap?: string;
}) {
  const images = (parts ?? []).filter(
    (p): p is Extract<ContentBlock, { type: "image" }> => p.type === "image",
  );
  if (images.length === 0) return null;
  const imgs = images.map((p, i) => (
    <img
      key={i}
      src={`data:${p.image.mimeType};base64,${p.image.data}`}
      alt=""
      className={className}
    />
  ));
  return wrap ? <div className={wrap}>{imgs}</div> : <>{imgs}</>;
}
