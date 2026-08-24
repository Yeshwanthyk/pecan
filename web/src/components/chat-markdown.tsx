/**
 * Chat markdown: GFM rendering with highlighted code fences and unified-diff
 * blocks (+/- line coloring) for ```diff fences. Diagram fences remain
 * inspectable source instead of pulling a multi-megabyte renderer into Pecan.
 */
import { memo, type ReactNode } from "react";
import ReactMarkdown, { defaultUrlTransform, type Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import { CopyButton } from "~/components/copy-button";
import { cn } from "~/lib/utils";

export const ChatMarkdown = memo(function ChatMarkdown({ text }: { text: string }) {
  return (
    <ReactMarkdown
      components={COMPONENTS}
      rehypePlugins={[rehypeHighlight]}
      remarkPlugins={[remarkGfm]}
      urlTransform={defaultUrlTransform}
    >
      {text}
    </ReactMarkdown>
  );
});

const COMPONENTS: Components = {
  pre: ({ children }) => <PreBlock>{children}</PreBlock>,
  code: CodeFence,
  a: ({ children, href }) => (
    <a
      className="text-foreground underline underline-offset-2 hover:opacity-70"
      href={href}
      rel="noreferrer noopener"
      target="_blank"
    >
      {children}
    </a>
  ),
  table: ({ children }) => (
    <div className="my-3 overflow-x-auto">
      <table className="w-full border-collapse text-[13px] [&_td,th]:border-border [&_td,th]:border [&_td,th]:px-2.5 [&_td,th]:py-1.5 [&_th]:bg-muted [&_th]:text-left">
        {children}
      </table>
    </div>
  ),
  blockquote: ({ children }) => (
    <blockquote className="my-3 border-l-2 border-border pl-3 text-muted-foreground">
      {children}
    </blockquote>
  ),
};

/** Routes diff fences to the specialized renderer and keeps all other code inspectable. */
function PreBlock({ children }: { children?: ReactNode }) {
  return <>{children}</>;
}

type CodeProps = {
  className?: string;
  children?: ReactNode;
};

function CodeFence({ className, children }: CodeProps) {
  const raw = String(children ?? "");
  const inline = !className?.includes("language-") && !raw.includes("\n");
  if (inline) {
    return (
      <code className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[12.5px]">
        {children}
      </code>
    );
  }
  const lang = /language-(\S+)/.exec(className ?? "")?.[1] ?? "";
  if (lang === "diff") return <DiffBlock code={raw} />;
  const codeText = raw.replace(/\n$/, "");
  return (
    <div className="group/code relative my-3 overflow-hidden rounded-lg border bg-card">
      <div className="flex items-center border-b bg-muted/50 px-3 py-1">
        <span className="font-mono text-[11px] text-muted-foreground">{lang || "code"}</span>
        <CopyButton
          className="ml-auto"
          text={() => codeText}
        />
      </div>
      <pre className="overflow-x-auto p-3 font-mono text-[12.5px] leading-relaxed">
        <code className={className}>{children}</code>
      </pre>
    </div>
  );
}

/** Unified diff renderer: hunk headers muted, + success tint, - destructive tint. */
export function DiffBlock({ code }: { code: string }) {
  const lines = code.replace(/\n$/, "").split("\n");
  return (
    <div className="my-3 overflow-hidden rounded-lg border bg-card">
      <div className="border-b bg-muted/50 px-3 py-1 font-mono text-[11px] text-muted-foreground">
        diff
      </div>
      <pre className="overflow-x-auto p-0 font-mono text-[12px] leading-[1.6]">
        {lines.map((line, i) => (
          <div
            className={cn(
              "px-3",
              line.startsWith("+") && line.startsWith("+++") === false && "bg-success/[0.08] text-success",
              line.startsWith("-") && line.startsWith("---") === false && "bg-destructive/[0.07] text-destructive",
              (line.startsWith("@@") || line.startsWith("---") || line.startsWith("+++")) &&
                "bg-muted/40 text-muted-foreground",
            )}
            key={i}
          >
            {line || " "}
          </div>
        ))}
      </pre>
    </div>
  );
}
