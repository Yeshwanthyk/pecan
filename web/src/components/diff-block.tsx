import { CopyButton } from "~/components/copy-button";
import { cn } from "~/lib/utils";

/** Unified diff renderer: hunk headers muted, + success tint, - destructive tint. */
export function DiffBlock({ code }: { code: string }) {
  const lines = code.replace(/\n$/, "").split("\n");
  return (
    <div className="not-prose my-3 overflow-hidden rounded-lg border bg-card">
      <div className="flex items-center border-b bg-muted/50 px-3 py-1 font-mono text-[11px] text-muted-foreground">
        <span>diff</span>
        <CopyButton className="ml-auto" label="Copy diff" text={code} />
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
