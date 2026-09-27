/**
 * Chat markdown: GFM rendering with highlighted code fences, unified-diff
 * blocks (+/- line coloring), and lazily rendered Mermaid diagrams.
 */
import { isValidElement, memo, type ReactNode } from "react";
import ReactMarkdown, { defaultUrlTransform, type Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import { CopyButton } from "~/components/copy-button";
import { DiffBlock } from "~/components/diff-block";
import { MermaidDiagram } from "~/components/mermaid-diagram";

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
    <div className="not-prose my-3 overflow-x-auto rounded-xl border px-1">
      <table className="my-0! w-full border-collapse text-[13px] [&_td,th]:border-b [&_td,th]:px-2.5 [&_td,th]:py-2 [&_th]:text-left [&_th]:font-medium [&_th]:text-muted-foreground [&_tr:last-child_td]:border-b-0">
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
  const raw = nodeText(children);
  const inline = !className?.includes("language-") && !raw.includes("\n");
  if (inline) {
    return (
      <code className="rounded-[5px] bg-muted px-1 py-px font-mono text-[0.86em]">
        {children}
      </code>
    );
  }
  const lang = /language-(\S+)/.exec(className ?? "")?.[1] ?? "";
  if (lang === "diff") return <DiffBlock code={raw} />;
  const codeText = raw.replace(/\n$/, "");
  if (lang === "mermaid") return <MermaidDiagram source={codeText} />;
  return (
    <div className="group/code not-prose relative my-3 overflow-hidden rounded-xl bg-muted">
      <div className="flex h-8 items-center ps-3.5 pe-1">
        <span className="text-[11px] font-medium text-muted-foreground">{lang || "code"}</span>
        <CopyButton
          className="ml-auto"
          text={() => codeText}
        />
      </div>
      <pre className="code-scaled overflow-x-auto px-3.5 pb-3 font-mono leading-relaxed">
        <code className={className}>{children}</code>
      </pre>
    </div>
  );
}

/** Extracts source text from highlighted React children without stringifying spans. */
function nodeText(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  if (isValidElement(node)) {
    const props = node.props as { children?: ReactNode };
    return nodeText(props.children);
  }
  return "";
}
