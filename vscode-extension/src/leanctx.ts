import { spawn } from "child_process";
import * as vscode from "vscode";

export interface KnowledgeFact {
  category: string;
  content: string;
  timestamp?: string;
}

export interface SessionStats {
  totalReads: number;
  totalSearches: number;
  totalShells: number;
  tokensSaved: number;
  sessionDuration: string;
  filesTouched: number;
}

export interface RepoMapEntry {
  path: string;
  rank: number;
  symbols: string[];
}

export interface SearchResult {
  file: string;
  line: number;
  content: string;
  score?: number;
}

function getBinaryPath(): string {
  return vscode.workspace
    .getConfiguration("leanctx")
    .get<string>("binaryPath", "lean-ctx");
}

export function runLeanCtx(
  args: string[],
  cwd?: string
): Promise<string> {
  return new Promise((resolve, reject) => {
    const bin = getBinaryPath();
    const workspaceCwd =
      cwd ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    const proc = spawn(bin, args, {
      cwd: workspaceCwd,
      env: { ...process.env, NO_COLOR: "1" },
      timeout: 30_000,
    });

    let stdout = "";
    let stderr = "";

    proc.stdout.on("data", (data: Buffer) => {
      stdout += data.toString();
    });

    proc.stderr.on("data", (data: Buffer) => {
      stderr += data.toString();
    });

    proc.on("error", (err: Error) => {
      reject(new Error(`Failed to run ${bin}: ${err.message}`));
    });

    proc.on("close", (code: number | null) => {
      if (code === 0) {
        resolve(stdout.trim());
      } else {
        reject(
          new Error(
            `${bin} exited with code ${code}: ${stderr || stdout}`.trim()
          )
        );
      }
    });
  });
}

export async function getSessionStats(): Promise<SessionStats> {
  try {
    const raw = await runLeanCtx(["metrics", "--json"]);
    const data = JSON.parse(raw);
    return {
      totalReads: data.total_reads ?? 0,
      totalSearches: data.total_searches ?? 0,
      totalShells: data.total_shells ?? 0,
      tokensSaved: data.tokens_saved ?? 0,
      sessionDuration: data.session_duration ?? "0s",
      filesTouched: data.files_touched ?? 0,
    };
  } catch {
    return {
      totalReads: 0,
      totalSearches: 0,
      totalShells: 0,
      tokensSaved: 0,
      sessionDuration: "—",
      filesTouched: 0,
    };
  }
}

export async function getKnowledge(): Promise<KnowledgeFact[]> {
  try {
    const raw = await runLeanCtx(["knowledge", "recall", "--json"]);
    const data = JSON.parse(raw);
    if (Array.isArray(data)) {
      return data.map((item: Record<string, string>) => ({
        category: item.category ?? "unknown",
        content: item.content ?? "",
        timestamp: item.timestamp,
      }));
    }
    return [];
  } catch {
    return [];
  }
}

export async function getRepoMap(): Promise<RepoMapEntry[]> {
  try {
    const raw = await runLeanCtx(["repomap", "--json"]);
    const data = JSON.parse(raw);
    if (Array.isArray(data)) {
      return data.map((item: Record<string, unknown>) => ({
        path: (item.path as string) ?? "",
        rank: (item.rank as number) ?? 0,
        symbols: Array.isArray(item.symbols)
          ? (item.symbols as string[])
          : [],
      }));
    }
    return [];
  } catch {
    return [];
  }
}

export async function semanticSearch(
  query: string
): Promise<SearchResult[]> {
  try {
    const raw = await runLeanCtx([
      "semantic-search",
      "--json",
      "--query",
      query,
    ]);
    const data = JSON.parse(raw);
    if (Array.isArray(data)) {
      return data.map((item: Record<string, unknown>) => ({
        file: (item.file as string) ?? "",
        line: (item.line as number) ?? 0,
        content: (item.content as string) ?? "",
        score: item.score as number | undefined,
      }));
    }
    return [];
  } catch {
    return [];
  }
}

export async function isAvailable(): Promise<boolean> {
  try {
    await runLeanCtx(["--version"]);
    return true;
  } catch {
    return false;
  }
}

export async function openVisualizer(): Promise<void> {
  await runLeanCtx(["visualize", "--open"]);
}

export async function getVersion(): Promise<string> {
  try {
    const raw = await runLeanCtx(["--version"]);
    return raw.replace(/^lean-ctx\s*/i, "").trim();
  } catch {
    return "unknown";
  }
}
