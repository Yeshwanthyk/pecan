import { Code2Icon, ImageIcon } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";

import { CopyButton } from "~/components/copy-button";
import { Spinner } from "~/components/ui/spinner";
import { useApp } from "~/store";

const MAX_SOURCE_LENGTH = 50_000;
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";

let mermaidPromise: Promise<typeof import("mermaid")["default"]> | undefined;

function loadMermaid() {
  mermaidPromise ??= import("mermaid").then(({ default: mermaid }) => mermaid);
  return mermaidPromise;
}

type RenderState =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; message: string };

const DIAGRAM_FONT =
  '-apple-system, BlinkMacSystemFont, "SF Pro Text", Inter, "Segoe UI", Roboto, sans-serif';

/** Quiet node palette matching the app's neutral surfaces. */
const LIGHT_DIAGRAM = {
  fontFamily: DIAGRAM_FONT,
  fontSize: "14px",
  primaryColor: "#f7f7f5",
  primaryBorderColor: "#dcdcd8",
  primaryTextColor: "#1c1c1c",
  secondaryColor: "#f0efec",
  tertiaryColor: "#ffffff",
  lineColor: "#a8a8a4",
  edgeLabelBackground: "#ffffff",
};

const DARK_DIAGRAM = {
  fontFamily: DIAGRAM_FONT,
  fontSize: "14px",
  darkMode: true,
  background: "#131313",
  primaryColor: "#1c1c1c",
  primaryBorderColor: "#333333",
  primaryTextColor: "#f2f2f2",
  secondaryColor: "#222222",
  tertiaryColor: "#131313",
  lineColor: "#6b6b6b",
  edgeLabelBackground: "#131313",
};

export function MermaidDiagram({ source }: { source: string }) {
  const theme = useApp((state) => state.theme);
  const reactId = useId();
  const diagramId = `pecan-mermaid-${reactId.replaceAll(/[^a-zA-Z0-9_-]/g, "")}`;
  const containerRef = useRef<HTMLDivElement>(null);
  const [renderState, setRenderState] = useState<RenderState>({ status: "loading" });
  const [showSource, setShowSource] = useState(false);

  useEffect(() => {
    let current = true;
    const target = containerRef.current;
    target?.replaceChildren();

    if (source.length > MAX_SOURCE_LENGTH) {
      setRenderState({
        status: "error",
        message: `Diagram source exceeds the ${MAX_SOURCE_LENGTH.toLocaleString()} character limit.`,
      });
      return () => {
        current = false;
      };
    }

    setRenderState({ status: "loading" });
    void loadMermaid()
      .then(async (mermaid) => {
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: "base",
          themeVariables: theme === "one-dark" ? DARK_DIAGRAM : LIGHT_DIAGRAM,
          themeCSS: ".node rect { rx: 8px; ry: 8px; } .edgeLabel { font-size: 12px; }",
          maxTextSize: MAX_SOURCE_LENGTH,
        });
        const { svg, bindFunctions } = await mermaid.render(diagramId, source);
        if (!current || target === null) return;
        mountSvg(target, svg);
        bindFunctions?.(target);
        setRenderState({ status: "ready" });
      })
      .catch((error: unknown) => {
        if (!current) return;
        target?.replaceChildren();
        setRenderState({ status: "error", message: renderErrorMessage(error) });
      });

    return () => {
      current = false;
    };
  }, [diagramId, source, theme]);

  return (
    <div
      className="not-prose my-3 overflow-hidden rounded-xl border bg-card"
      data-state={renderState.status}
      data-testid="mermaid"
    >
      <div className="flex h-8 items-center ps-3.5 pe-1">
        <span className="text-[11px] font-medium text-muted-foreground">diagram</span>
        <div className="ml-auto flex items-center gap-0.5">
          {renderState.status === "ready" ? (
            <button
              aria-label={showSource ? "Show diagram" : "Show Mermaid source"}
              className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              onClick={() => setShowSource((visible) => !visible)}
              title={showSource ? "Show diagram" : "Show source"}
              type="button"
            >
              {showSource ? <ImageIcon className="size-3.5" /> : <Code2Icon className="size-3.5" />}
            </button>
          ) : null}
          <CopyButton label="Copy Mermaid source" text={source} />
        </div>
      </div>

      <div
        aria-busy={renderState.status === "loading"}
        className={showSource ? "hidden" : "relative min-h-28"}
      >
        <div
          className="overflow-x-auto p-4 [&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full"
          ref={containerRef}
        />
        {renderState.status === "loading" ? (
          <div className="absolute inset-0 flex items-center justify-center gap-2 text-[12px] text-muted-foreground">
            <Spinner className="size-3.5" />
            Rendering diagram
          </div>
        ) : null}
        {renderState.status === "error" ? (
          <div className="flex min-h-28 flex-col items-center justify-center gap-1 px-4 py-6 text-center">
            <span className="text-[13px] font-medium">Could not render diagram</span>
            <span className="max-w-xl text-[12px] text-muted-foreground">
              {renderState.message}
            </span>
          </div>
        ) : null}
      </div>

      {showSource || renderState.status === "error" ? (
        <pre className="overflow-x-auto p-3 font-mono text-[12.5px] leading-relaxed">
          <code>{source}</code>
        </pre>
      ) : null}
    </div>
  );
}

function mountSvg(target: HTMLElement, source: string) {
  const parsed = new DOMParser().parseFromString(source, "image/svg+xml");
  const root = parsed.documentElement;
  if (root.localName !== "svg" || root.namespaceURI !== SVG_NAMESPACE) {
    throw new Error("Mermaid returned an invalid SVG document.");
  }
  target.replaceChildren(document.importNode(root, true));
}

function renderErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim() !== "") return error.message;
  return "Check the Mermaid syntax and try again.";
}
