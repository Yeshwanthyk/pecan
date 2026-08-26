/**
 * Dialog fixture extension for the pecan extension-bridge proof.
 *
 * Registers `/fixture-dialogs`, a command that emits every blocking
 * extension dialog Pi supports — `select`, `confirm`, `input`, `editor`,
 * plus a second `select` answered by cancelling — and records the resolved
 * values exactly as `ctx.ui.*` returns them. The RPC client (see
 * `scripts/prove-dialog-bridge.sh`) answers each `extension_ui_request`
 * frame on stdin with an `extension_ui_response` echoing its `id`, exactly
 * like Pecan's `/respond` endpoint does.
 *
 * The extension itself never calls the model, so the proof runs without any
 * LLM request: Pi only runs this command handler and the RPC dialog
 * sub-protocol.
 *
 * Usage (from the repo root):
 *   pi --mode rpc --no-session --no-extensions \
 *      --extension scripts/dialog-fixture-extension.ts
 *   ... then send: {"type":"prompt","message":"/fixture-dialogs /tmp/out.json"}
 *
 * The results file path is the first argument to the command, or
 * `$DIALOG_FIXTURE_OUT`, or `/tmp/pecan-dialog-fixture-results.json`.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	pi.registerCommand("fixture-dialogs", {
		description:
			"Emit select/confirm/input/editor dialogs and record their resolutions (extension-bridge proof)",
		handler: async (args, ctx) => {
			const out = args.trim().split(/\s+/)[0] ||
				process.env.DIALOG_FIXTURE_OUT ||
				"/tmp/pecan-dialog-fixture-results.json";

			const results: Record<string, unknown> = {
				select: await ctx.ui.select("Pick an option", ["Allow", "Block"]),
				confirm: await ctx.ui.confirm("Clear session?", "All messages will be lost."),
				input: await ctx.ui.input("Enter a value", "type something..."),
				editor: await ctx.ui.editor("Edit some text", "Line 1\nLine 2"),
				cancelled: await ctx.ui.select("Pick again", ["A", "B"]),
			};

			const fs = await import("node:fs");
			fs.writeFileSync(out, JSON.stringify(results, null, 2));
			ctx.ui.notify(`Wrote ${out}`, "info");
		},
	});
}