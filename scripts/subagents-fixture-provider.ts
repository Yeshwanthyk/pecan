/**
 * Deterministic script-provider for the real pi-subagents bridge proof.
 *
 * Loaded into a real `pi --mode rpc` child together with the REAL local
 * pi-subagents package (`--no-extensions` + explicit `--extension` loads,
 * so nothing else from the user's settings interferes). It plays the role
 * the parent LLM would:
 *
 *   turn 1  -> `subagent_spawn` (harness "pi", one child, a dedicated
 *              `fixture-child-v1` model) — the extension starts the child
 *   turn 2  -> `subagent_wait` on the spawned id — blocks until the child
 *              settles and returns its final output
 *   turn 3  -> settles with plain text
 *
 * The child itself does NOT stream through this provider: pi-subagents runs
 * Pi children in-process via the SDK, and each child builds a fresh model
 * runtime that only sees the agent dir's `auth.json` / `models.json` (not
 * in-memory extension registrations). The proof therefore seeds a temp agent
 * dir (`PI_CODING_AGENT_DIR`) declaring `pecan-fixture` as an
 * openai-completions provider whose baseUrl is a local offline mock served
 * by the proof driver — the same route real providers take. This provider's
 * own baseUrl must point at that same mock for the model object the child
 * inherits, hence `PECAN_FIXTURE_PORT`.
 *
 * After each `subagent_spawn` / `subagent_wait` tool result lands in
 * context, the provider records the exact model-facing result text (the
 * same text pecan's web host would show and the parent would read) as one
 * JSON line in `$SUBAGENTS_FIXTURE_OUT` (default
 * `/tmp/pecan-subagents-proof-results.jsonl`). The driver cross-checks this
 * file against the RPC events. No network is ever touched.
 */
import type { ExtensionAPI, Message } from "@earendil-works/pi-coding-agent";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

const OUT =
	process.env.SUBAGENTS_FIXTURE_OUT ?? "/tmp/pecan-subagents-proof-results.jsonl";
/** Local offline mock port wired in by the proof driver; see file header. */
const PORT = process.env.PECAN_FIXTURE_PORT ?? "9";

/** The child's scripted prompt and the reply the offline mock returns. */
const CHILD_PROMPT = "Reply with exactly the text: subagent done";

function spawnResults(messages: Message[]) {
	return messages.filter(
		(message): message is Extract<Message, { role: "toolResult" }> =>
			message.role === "toolResult" && message.toolName === "subagent_spawn",
	);
}

function waitResults(messages: Message[]) {
	return messages.filter(
		(message): message is Extract<Message, { role: "toolResult" }> =>
			message.role === "toolResult" && message.toolName === "subagent_wait",
	);
}

function resultText(message: { content: Array<{ type: string; text?: string }> }): string {
	return message.content
		.map((block) => (block.type === "text" && block.text ? block.text : ""))
		.filter(Boolean)
		.join("\n");
}

/** The id pi-subagents embeds in the spawn result text, e.g. `sa-1`. */
function spawnId(text: string): string | undefined {
	const match = /Spawned subagent (sa-\d+)/.exec(text);
	return match?.[1];
}

/** Counts spawn results already recorded on disk so appends are idempotent. */
let recordedSpawns = 0;

export default function (pi: ExtensionAPI) {
	pi.registerProvider("pecan-fixture", {
		name: "Pecan Fixture",
		baseUrl: `http://127.0.0.1:${PORT}/v1`,
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
			// Resolvable by the parent registry so `subagent_spawn` can name
			// it explicitly; the child itself streams through the seeded
			// provider in the temp agent dir (see file header), not here.
			{
				id: "fixture-child-v1",
				name: "Fixture Child",
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
				// Only the parent runs through this provider in practice; the
				// child uses the seeded offline provider. Still, never recurse:
				// a child-visible call settles immediately instead of looping.
				if (model.id !== "fixture-v1") {
					stream.push({
						type: "done",
						reason: "stop",
						message: fixtureMessage(
							model.id,
							"subagent done",
							"stop",
						),
					});
					stream.end();
					return;
				}

				const spawns = spawnResults(context.messages);
				const waits = waitResults(context.messages);
				if (spawns.length > recordedSpawns) {
					const fresh = spawns.slice(recordedSpawns);
					recordedSpawns = spawns.length;
					const lines = fresh.map((result) =>
						JSON.stringify({ kind: "parent-spawn-result", text: resultText(result) }),
					);
					const fs = await import("node:fs");
					fs.appendFileSync(OUT, lines.join("\n") + "\n");
				}

				if (waits.length > 0) {
					const last = waits[waits.length - 1];
					const fs = await import("node:fs");
					fs.appendFileSync(
						OUT,
						JSON.stringify({ kind: "parent-wait-result", text: resultText(last) }) + "\n",
					);
					stream.push({
						type: "done",
						reason: "stop",
						message: fixtureMessage(model.id, "parent proof complete", "stop"),
					});
					stream.end();
					return;
				}

				if (spawns.length > 0) {
					const id = spawnId(resultText(spawns[spawns.length - 1]));
					// A failed/unknown id would make subagent_wait throw; settle
					// without echoing an id back at the tool contract.
					if (id === undefined) {
						stream.push({
							type: "done",
							reason: "stop",
							message: fixtureMessage(model.id, "parent proof complete", "stop"),
						});
						stream.end();
						return;
					}
					stream.push({
						type: "start",
						partial: fixtureMessage(
							model.id,
							"",
							"pending",
							[
								{
									type: "toolCall",
									id: "fixture-wait",
									name: "subagent_wait",
									arguments: { ids: [id] },
								},
							],
						),
					});
					stream.push({
						type: "done",
						reason: "toolUse",
						message: fixtureMessage(
							model.id,
							"",
							"toolUse",
							[
								{
									type: "toolCall",
									id: "fixture-wait",
									name: "subagent_wait",
									arguments: { ids: [id] },
								},
							],
						),
					});
					stream.end();
					return;
				}

				stream.push({
					type: "start",
					partial: fixtureMessage(
						model.id,
						"",
						"pending",
						[
							{
								type: "toolCall",
								id: "fixture-spawn",
								name: "subagent_spawn",
								arguments: {
									prompt: CHILD_PROMPT,
									name: "Fixture child",
									harness: "pi",
									model: "fixture-child-v1",
								},
							},
						],
					),
				});
				stream.push({
					type: "done",
					reason: "toolUse",
					message: fixtureMessage(
						model.id,
						"",
						"toolUse",
						[
							{
								type: "toolCall",
								id: "fixture-spawn",
								name: "subagent_spawn",
								arguments: {
									prompt: CHILD_PROMPT,
									name: "Fixture child",
									harness: "pi",
									model: "fixture-child-v1",
								},
							},
						],
					),
				});
				stream.end();
			})();
			return stream;
		},
	});
}

/** Builds a minimal assistant message in the shape pi's RPC expects. */
function fixtureMessage(
	model: string,
	text: string,
	stopReason: "pending" | "toolUse" | "stop",
	content?: Array<{ type: "toolCall"; id: string; name: string; arguments: unknown }>,
) {
	const blocks: Array<{ type: "text"; text: string } | { type: "toolCall"; id: string; name: string; arguments: unknown }> =
		text ? [{ type: "text", text }] : [];
	if (content) blocks.push(...content);
	return {
		role: "assistant" as const,
		content: blocks,
		api: "openai-completions" as const,
		provider: "pecan-fixture" as const,
		model,
		usage: {
			input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason,
		timestamp: Date.now(),
	};
}