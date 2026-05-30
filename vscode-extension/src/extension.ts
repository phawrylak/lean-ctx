import * as vscode from "vscode";
import { SidebarProvider } from "./sidebar/provider";
import { StatusBarManager } from "./statusbar";
import { registerCommands } from "./commands";
import { isAvailable } from "./leanctx";

let statusBar: StatusBarManager | undefined;

export async function activate(
  context: vscode.ExtensionContext
): Promise<void> {
  const available = await isAvailable();
  if (!available) {
    vscode.window.showWarningMessage(
      'lean-ctx binary not found. Install lean-ctx or set "leanctx.binaryPath" in settings.'
    );
  }

  const sidebarProvider = new SidebarProvider(context.extensionUri);

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(
      SidebarProvider.viewType,
      sidebarProvider
    )
  );

  statusBar = new StatusBarManager();
  context.subscriptions.push({ dispose: () => statusBar?.dispose() });
  statusBar.start();

  registerCommands(context, sidebarProvider);
}

export function deactivate(): void {
  statusBar?.dispose();
  statusBar = undefined;
}
