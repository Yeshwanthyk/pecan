/**
 * Deterministic Pi model for Pecan verification.
 *
 * Loaded into a real `pi --mode rpc` worker with `--no-extensions --extension
 * scripts/fixture-model.ts --provider pecan-fixture --model fixture-v1`. It
 * plays the model's role without touching the network, so every Pecan flow
 * (create, send, stream, steer, queue, abort, tools, dialogs, rendering) can
 * be driven and measured from the CLI.
 *
 * The first word of the latest user message selects a scenario:
 *
 *   (anything)  stream "Echo: <message>" word by word
 *   /slow       stream ~6s of words (room to steer or abort)
 *   /tool       call the built-in `bash` tool, then summarize its output
 *   /confirm    call `fixture_confirm`, which opens a confirm dialog
 *   /think      emit a thinking block, then text
 *   /rich       emit markdown: headings, list, table, code, mermaid
 *   /error      fail the turn with a provider error
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import { Type } from "@sinclair/typebox";

type Block =
	| { type: "text"; text: string }
	| { type: "thinking"; thinking: string }
	| { type: "toolCall"; id: string; name: string; arguments: Record<string, unknown> };

type Plan = { thinking?: string; text?: string; tool?: { name: string; arguments: Record<string, unknown> }; error?: string; delayMs: number };

const RICH = [
	"## Fixture report",
	"",
	"Streaming, tools, and rendering all check out.",
	"",
	"- first item with `inline code`",
	"- second item with **bold** text",
	"",
	"| metric | value |",
	"| --- | --- |",
	"| sessions | 3 |",
	"| latency | 12 ms |",
	"",
	"```rust",
	"fn main() {",
	'    println!("hello from the fixture");',
	"}",
	"```",
	"",
	"```mermaid",
	"graph LR; CLI-->Pecan; Pecan-->Pi",
	"```",
].join("\n");

function textOf(content: unknown): string {
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return "";
	return content
		.map((block) => (block && typeof block === "object" && "text" in block ? String(block.text ?? "") : ""))
		.join("");
}

function plan(messages: Array<{ role: string; content: unknown; toolName?: string }>, callCount: number): Plan {
	const last = messages.at(-1);
	if (last?.role === "toolResult") {
		const output = textOf(last.content).trim();
		return { text: `Tool ${last.toolName ?? "call"} returned: ${output}`, delayMs: 15 };
	}
	const prompt = textOf([...messages].reverse().find((message) => message.role === "user")?.content).trim();
	const [command, ...rest] = prompt.split(/\s+/);
	const tail = rest.join(" ");
	switch (command) {
		case "/slow":
			return { text: Array.from({ length: 40 }, (_, i) => `word${i + 1}`).join(" "), delayMs: 150 };
		case "/tool":
			return { text: "Running a command.", tool: { name: "bash", arguments: { command: "echo fixture-tool-ok" } }, delayMs: 15 };
		case "/confirm":
			return { text: "Asking first.", tool: { name: "fixture_confirm", arguments: { question: tail || "Proceed?" } }, delayMs: 15 };
		case "/think":
			return { thinking: "Considering the request carefully.", text: `Thought about: ${tail || "nothing"}`, delayMs: 15 };
		case "/rich":
			return { text: RICH, delayMs: 5 };
		case "/error":
			return { error: "fixture provider error", delayMs: 0 };
		default:
			return { text: `Echo: ${prompt}`, delayMs: 20 };
	}
	void callCount;
}

const sleep = (ms: number, signal?: AbortSignal) =>
	new Promise<void>((resolve) => {
		const timer = setTimeout(resolve, ms);
		signal?.addEventListener("abort", () => {
			clearTimeout(timer);
			resolve();
		});
	});

export default function (pi: ExtensionAPI) {
	let calls = 0;

	pi.registerTool({
		name: "fixture_confirm",
		label: "Fixture confirm",
		description: "Ask the user a yes/no question.",
		parameters: Type.Object({ question: Type.String() }),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const ok = await ctx.ui.confirm("Fixture confirm", params.question);
			return { content: [{ type: "text", text: ok ? "confirmed" : "declined" }], details: { ok } };
		},
	});

	pi.registerProvider("pecan-fixture", {
		name: "Pecan Fixture",
		baseUrl: "http://127.0.0.1:9/v1", // unused: streamSimple never connects
		apiKey: "fixture-key",
		api: "openai-completions",
		models: [
			{
				id: "fixture-v1",
				name: "Fixture Model",
				reasoning: true,
				input: ["text", "image"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 64000,
				maxTokens: 4096,
			},
		],
		streamSimple(model, context, options) {
			const stream = createAssistantMessageEventStream();
			const signal: AbortSignal | undefined = options?.signal;
			calls += 1;
			const step = plan(context.messages as never, calls);
			const content: Block[] = [];
			const output = {
				role: "assistant" as const,
				content,
				api: "openai-completions" as const,
				provider: "pecan-fixture" as const,
				model: model.id,
				usage: {
					input: 10, output: 10, cacheRead: 0, cacheWrite: 0, totalTokens: 20,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				},
				stopReason: "stop" as "stop" | "toolUse" | "error" | "aborted",
				errorMessage: undefined as string | undefined,
				timestamp: Date.now(),
			};
			const partial = () => output as never;

			void (async () => {
				stream.push({ type: "start", partial: partial() });
				if (step.error) {
					output.stopReason = "error";
					output.errorMessage = step.error;
					stream.push({ type: "error", reason: "error", error: partial() });
					stream.end();
					return;
				}
				if (step.thinking) {
					const index = content.push({ type: "thinking", thinking: "" }) - 1;
					stream.push({ type: "thinking_start", contentIndex: index, partial: partial() });
					(content[index] as { thinking: string }).thinking = step.thinking;
					stream.push({ type: "thinking_delta", contentIndex: index, delta: step.thinking, partial: partial() });
					stream.push({ type: "thinking_end", contentIndex: index, content: step.thinking, partial: partial() });
				}
				if (step.text) {
					const index = content.push({ type: "text", text: "" }) - 1;
					const block = content[index] as { text: string };
					stream.push({ type: "text_start", contentIndex: index, partial: partial() });
					for (const piece of step.text.split(/(?<=\s)/)) {
						if (signal?.aborted) break;
						await sleep(step.delayMs, signal);
						block.text += piece;
						stream.push({ type: "text_delta", contentIndex: index, delta: piece, partial: partial() });
					}
					stream.push({ type: "text_end", contentIndex: index, content: block.text, partial: partial() });
				}
				if (signal?.aborted) {
					output.stopReason = "aborted";
					output.errorMessage = "aborted";
					stream.push({ type: "error", reason: "aborted", error: partial() });
					stream.end();
					return;
				}
				if (step.tool) {
					const toolCall = { type: "toolCall" as const, id: `fixture-call-${calls}`, ...step.tool };
					const index = content.push(toolCall) - 1;
					stream.push({ type: "toolcall_start", contentIndex: index, partial: partial() });
					stream.push({ type: "toolcall_end", contentIndex: index, toolCall, partial: partial() });
					output.stopReason = "toolUse";
				}
				stream.push({ type: "done", reason: output.stopReason as "stop" | "toolUse", message: partial() });
				stream.end();
			})();
			return stream;
		},
	});
}
