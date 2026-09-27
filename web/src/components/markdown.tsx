/**
 * Lazy entry point for chat markdown. The renderer (react-markdown, GFM,
 * highlight.js) is the heaviest part of the bundle, so it loads in its own
 * chunk: prefetched at idle after first paint, with plain text shown until it
 * arrives.
 */
import { lazy, memo, Suspense } from "react";

const loadRenderer = () => import("~/components/chat-markdown");

const Renderer = lazy(() => loadRenderer().then((module) => ({ default: module.ChatMarkdown })));

export const ChatMarkdown = memo(function ChatMarkdown({ text }: { text: string }) {
  return (
    <Suspense fallback={<p className="whitespace-pre-wrap">{text}</p>}>
      <Renderer text={text} />
    </Suspense>
  );
});

/** Starts fetching the renderer once the browser is idle after first paint. */
export function prefetchMarkdown() {
  const start = () => void loadRenderer();
  if ("requestIdleCallback" in window) window.requestIdleCallback(start, { timeout: 2000 });
  else setTimeout(start, 200);
}
