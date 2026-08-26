/**
 * Deterministic script-provider for the real pi-askuser bridge proof.
 *
 * Loaded into a real `pi --mode rpc` child together with the real
 * pi-askuser package (`--no-extensions` + explicit `--extension` loads, so
 * nothing else from the user's settings interferes). It plays the role the
 * LLM would: on the first turn it issues one `ask_user` tool call with a
 * scripted batch (duplicate labels, optional question, multi-select, custom
 * answer), on the second a second `ask_user` call that the driver cancels,
 * then it settles with plain text. No network is ever touched.
 *
 * After each `ask_user` tool result lands in context, the provider records
 * the exact model-facing result text (the same text pecan's web host would
 * show and the parent model would read) as one JSON line in
 * `$ASKUSER_FIXTURE_OUT` (default `/tmp/pecan-askuser-proof-results.jsonl`).
 * The driver verifies both the dialog frames and this file.
 */
import type { ExtensionAPI, Message } from "@earendil-works/pi-coding-agent";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

const OUT =
	process.env.ASKUSER_FIXTURE_OUT ?? "/tmp/pecan-askuser-proof-results.jsonl";

/** First call: four sequential questions covering the fallback semantics. */
const FIRST_CALL = {
	context: "Shared: the user already reviewed the plan",
	questions: [
		// Duplicate label: forces \u2063-tagged generated select values.
		{ id: "mode", question: "Pick a mode", options: [{ label: "Allow" }, { label: "Block" }, { label: "Allow" }] },
		// Optional single-select: exposes the skip row.
		{ id: "scope", question: "Where should it apply?", optional: true, options: [{ label: "Here" }, { label: "There" }] },
		// Multi-select: checkbox toggles + Done.
		{ id: "features", question: "Which features?", multiSelect: true, options: [{ label: "Search" }, { label: "Diff" }, { label: "Ship" }] },
		// Standard single-select answered via the write-your-own path.
		{ id: "nickname", question: "Name the bot", options: [{ label: "Pecan" }, { label: "Walnut" }] },
	],
};

/** Second call: two questions; the driver cancels mid-batch (dismissed). */
const SECOND_CALL = {
	questions: [
		{ id: "first-confirm", question: "First confirm", options: [{ label: "Yes" }, { label: "No" }] },
		{ id: "second-confirm", question: "Second confirm", options: [{ label: "Okay" }, { label: "Nope" }] },
	],
};

function askUserResults(messages: Message[]) {
	return messages.filter(
		(message): message is Extract<Message, { role: "toolResult" }> =>
			message.role === "toolResult" && message.toolName === "ask_user",
	);
}

function resultText(message: { content: Array<{ type: string; text?: string }> }): string {
	return message.content
		.map((block) => (block.type === "text" && block.text ? block.text : ""))
		.filter(Boolean)
		.join("\n");
}

/** Counts results already recorded on disk so appends are idempotent. */
let recorded = 0;

export default function (pi: ExtensionAPI) {
	pi.registerProvider("pecan-fixture", {
		name: "Pecan Fixture",
		baseUrl: "http://127.0.0.1:9/v1", // unused: streamSimple never connects
		apiKey: "fixture-key",
		api: "openai-completions",
		models: [
			{
				id: "fixture-v1",
				name: "Fixture Model",
				reasoning: false,
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 64000,
				maxTokens: 4096,
			},
		],
		async streamSimple(model, context) {
			const stream = createAssistantMessageEventStream();
			void (async () => {
				const results = askUserResults(context.messages);
				if (results.length > recorded) {
					const next = results.slice(recorded);
					recorded = results.length;
					const lines = next.map((result, offset) =>
						JSON.stringify({
							call: recorded - next.length + offset + 1,
							text: resultText(result),
						}),
					);
					const fs = await import("node:fs");
					fs.appendFileSync(OUT, lines.join("\n") + "\n");
				}

				const output = {
					role: "assistant" as const,
					content: [] as Array<{ type: "text"; text: string } | { type: "toolCall"; id: string; name: string; arguments: unknown }>,
					api: "openai-completions" as const,
					provider: "pecan-fixture" as const,
					model: model.id,
					usage: {
						input: 0, output: 0, cacheRead: 0, cacheWrite: 0,
						totalTokens: 0,
						cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
					},
					stopReason: "pending" as const,
					timestamp: Date.now(),
				};

				if (results.length === 0) {
					output.content.push({
						type: "toolCall",
						id: "fixture-call-1",
						name: "ask_user",
						arguments: FIRST_CALL,
					});
					output.stopReason = "toolUse";
				} else if (results.length === 1) {
					output.content.push({
						type: "toolCall",
						id: "fixture-call-2",
						name: "ask_user",
						arguments: SECOND_CALL,
					});
					output.stopReason = "toolUse";
				} else {
					output.content.push({ type: "text", text: "proof complete" });
					output.stopReason = "stop";
				}

				stream.push({ type: "start", partial: output });
				stream.push({ type: "done", reason: output.stopReason, message: output });
				stream.end();
			})();
			return stream;
		},
	});
}