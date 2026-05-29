import { LeanCtxClient, LeanCtxOptions } from "./client";

/**
 * Create a Vercel AI SDK compatible tool that wraps lean-ctx search.
 * Usage with `ai` package:
 *
 * ```ts
 * import { generateText } from 'ai';
 * import { createLeanCtxTool } from 'lean-ctx';
 *
 * const result = await generateText({
 *   model: myModel,
 *   tools: { search: createLeanCtxTool() },
 *   prompt: 'Find the auth implementation',
 * });
 * ```
 */
export function createLeanCtxTool(options?: LeanCtxOptions) {
  const client = new LeanCtxClient(options);

  return {
    description: "Search code using lean-ctx hybrid search (BM25 + vector + SPLADE)",
    parameters: {
      type: "object" as const,
      properties: {
        query: {
          type: "string" as const,
          description: "The search query",
        },
        path: {
          type: "string" as const,
          description: "Optional path scope",
        },
      },
      required: ["query"] as const,
    },
    execute: async ({ query, path }: { query: string; path?: string }) => {
      return client.search(query, path);
    },
  };
}
