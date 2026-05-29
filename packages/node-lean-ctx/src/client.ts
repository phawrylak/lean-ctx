import { execSync } from "child_process";
import { resolve } from "path";

export interface LeanCtxOptions {
  binary?: string;
  projectRoot?: string;
  timeout?: number;
}

export class LeanCtxClient {
  private binary: string;
  private projectRoot: string;
  private timeout: number;

  constructor(options: LeanCtxOptions = {}) {
    this.binary = options.binary ?? "lean-ctx";
    this.projectRoot = options.projectRoot ?? process.cwd();
    this.timeout = options.timeout ?? 30000;
  }

  read(path: string, mode: string = "auto"): string {
    return this.run(["read", path, "--mode", mode]);
  }

  search(pattern: string, path?: string): string {
    const args = ["grep", pattern];
    if (path) args.push(path);
    return this.run(args);
  }

  shell(command: string): string {
    return this.run(["-c", command]);
  }

  gain(): Record<string, unknown> {
    const output = this.run(["gain", "--json"]);
    try {
      return JSON.parse(output);
    } catch {
      return { raw: output };
    }
  }

  benchmark(
    path?: string,
    jsonOutput: boolean = true
  ): Record<string, unknown> {
    const args = ["benchmark", "eval"];
    if (path) args.push(path);
    if (jsonOutput) args.push("--json");
    const output = this.run(args);
    try {
      return JSON.parse(output);
    } catch {
      return { raw: output };
    }
  }

  private run(args: string[]): string {
    try {
      const result = execSync([this.binary, ...args].join(" "), {
        cwd: this.projectRoot,
        timeout: this.timeout,
        encoding: "utf-8",
        stdio: ["pipe", "pipe", "pipe"],
      });
      return result.trim();
    } catch (error: unknown) {
      if (
        error instanceof Error &&
        "code" in error &&
        (error as NodeJS.ErrnoException).code === "ENOENT"
      ) {
        throw new Error(
          `lean-ctx binary not found at '${this.binary}'. ` +
            "Install: curl -fsSL https://leanctx.com/install.sh | sh"
        );
      }
      throw error;
    }
  }
}
