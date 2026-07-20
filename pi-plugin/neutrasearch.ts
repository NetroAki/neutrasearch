import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** One MCP process per Pi runtime: index decode happens once, then each query
 * is one JSON line in + one compact JSON line out. */
class PersistentNeutraClient {
  private child?: ChildProcessWithoutNullStreams;
  private buffer = "";
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  private starting?: Promise<void>;

  private async ensure(): Promise<void> {
    if (this.child && !this.child.killed) return;
    if (this.starting) return this.starting;
    this.starting = new Promise<void>((resolve, reject) => {
      const command =
        process.env.NEUTRASEARCH_MCP ?? process.env.NEUTRA_MCP ?? "neutrasearch-mcp";
      const child = spawn(command, [], { stdio: ["pipe", "pipe", "pipe"] });
      this.child = child;
      child.stdout.setEncoding("utf8").on("data", (chunk: string) => this.onData(chunk));
      child.stderr.setEncoding("utf8").on("data", () => {}); // protocol stays stdout-only
      child.on("error", reject);
      child.on("close", (code) => {
        const error = new Error(`neutrasearch-mcp exited ${code ?? "by signal"}`);
        for (const waiter of this.pending.values()) waiter.reject(error);
        this.pending.clear();
        this.child = undefined;
      });
      this.requestRaw("initialize", {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: "pi-neutrasearch", version: "0.1.0" },
      }).then(() => resolve(), reject);
    }).finally(() => { this.starting = undefined; });
    return this.starting;
  }

  private onData(chunk: string) {
    this.buffer += chunk;
    for (;;) {
      const newline = this.buffer.indexOf("\n");
      if (newline < 0) break;
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      if (!line) continue;
      try {
        const message = JSON.parse(line);
        const waiter = this.pending.get(message.id);
        if (!waiter) continue;
        this.pending.delete(message.id);
        if (message.error) waiter.reject(new Error(message.error.message));
        else waiter.resolve(message.result);
      } catch { /* malformed server line is ignored; pending request times out via abort */ }
    }
  }

  private requestRaw(method: string, params: unknown): Promise<any> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      if (!this.child) return reject(new Error("neutrasearch-mcp is not running"));
      this.pending.set(id, { resolve, reject });
      this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  }

  async search(query: string, limit: number, metadata: boolean, signal?: AbortSignal): Promise<string> {
    await this.ensure();
    const request = this.requestRaw("tools/call", { name: "neutra_search", arguments: { query, limit, metadata } });
    if (!signal) return (await request).content[0].text;
    return await Promise.race([
      request.then((result) => result.content[0].text as string),
      new Promise<string>((_, reject) => signal.addEventListener("abort", () => reject(new Error("neutra_search aborted")), { once: true })),
    ]);
  }

  stop() { this.child?.kill("SIGTERM"); }
}

const client = new PersistentNeutraClient();

const tool = defineTool({
  name: "neutra_search",
  label: "Neutrasearch",
  description: "Token-efficient resident-index filename/path search. Prefer over grep/find/rg --files for locating files; no filesystem walk or per-call index load. Content search is intentionally out of scope.",
  promptSnippet: "Use neutra_search first for fast path discovery; use content grep only on the narrowed files",
  promptGuidelines: [
    "Keep limits small (default 50); request metadata only when it affects the decision.",
    "Filters: ext:rs kind:file|dir fs:btrfs|ext4|ntfs|zfs size:>1M under:/path.",
  ],
  parameters: Type.Object({
    query: Type.String({ description: "Filename/path query plus optional filters" }),
    limit: Type.Optional(Type.Number({ minimum: 1, maximum: 1000, description: "Result cap; default 50" })),
    metadata: Type.Optional(Type.Boolean({ description: "Include metadata columns; default false (paths only)" })),
  }),
  async execute(_id, params, signal) {
    const limit = Math.min(1000, Math.max(1, params.limit ?? 50));
    const text = await client.search(params.query, limit, params.metadata === true, signal);
    return { content: [{ type: "text" as const, text }], details: { query: params.query, limit } };
  },
});

export default function neutrasearch(pi: ExtensionAPI) {
  pi.registerTool(tool);
  process.once("exit", () => client.stop());
}
