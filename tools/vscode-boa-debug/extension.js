// Minimal extension entry point for Boa debugger
// This extension provides DAP (Debug Adapter Protocol) support for debugging
// JavaScript code with the Boa engine.

const vscode = require('vscode');
const path = require('path');
const { spawn } = require('child_process');

/**
 * @param {vscode.ExtensionContext} context
 */
function activate(context) {
    console.log('='.repeat(60));
    console.log('[BOA EXTENSION] 🚀 Activation starting...');
    console.log('[BOA EXTENSION] Extension path:', context.extensionPath);
    console.log('='.repeat(60));

    try {
        // Register a debug adapter descriptor factory
        console.log('[BOA EXTENSION] Registering debug adapter factory...');
        const factory = new BoaDebugAdapterDescriptorFactory();
        const factoryDisposable = vscode.debug.registerDebugAdapterDescriptorFactory('boa', factory);
        context.subscriptions.push(factoryDisposable);
        console.log('[BOA EXTENSION] ✓ Debug adapter factory registered');

        // Register a configuration provider for dynamic configurations
        console.log('[BOA EXTENSION] Registering configuration provider...');
        const provider = new BoaConfigurationProvider();
        const providerDisposable = vscode.debug.registerDebugConfigurationProvider('boa', provider);
        context.subscriptions.push(providerDisposable);
        console.log('[BOA EXTENSION] ✓ Configuration provider registered');

        console.log('='.repeat(60));
        console.log('[BOA EXTENSION] ✅ Extension activated successfully!');
        console.log('[BOA EXTENSION] Ready to debug JavaScript with Boa');
        console.log('='.repeat(60));

        // Show a notification
        vscode.window.showInformationMessage('Boa Debugger: Extension activated! Ready to debug.');

    } catch (error) {
        console.error('[BOA EXTENSION] ❌ Activation failed:', error);
        vscode.window.showErrorMessage(`Boa Debugger activation failed: ${error.message}`);
        throw error;
    }
}

function deactivate() {
    console.log('Boa debugger extension deactivated');
}

/**
 * Factory for creating debug adapter descriptors
 */
class BoaDebugAdapterDescriptorFactory {
    /**
     * @param {vscode.DebugSession} session
     * @returns {vscode.ProviderResult<vscode.DebugAdapterDescriptor>}
     */
    createDebugAdapterDescriptor(session) {
        console.log(`[Boa Debug] Creating debug adapter for session: ${session.name}`);
        console.log(`[Boa Debug] Configuration:`, session.configuration);

        // Path to the boa-cli executable
        const boaCliPath = this.findBoaCli();

        if (!boaCliPath) {
            const errorMsg = 'boa-cli not found. Please ensure it is built in target/debug or target/release.';
            console.error(`[Boa Debug] ${errorMsg}`);
            vscode.window.showErrorMessage(errorMsg);
            return null;
        }

        console.log(`[Boa Debug] Using boa-cli at: ${boaCliPath}`);

        // Launch boa-cli with --dap flag to start DAP server over stdio
        const descriptor = new vscode.DebugAdapterExecutable(
            boaCliPath,
            ['--dap'],
            {
                cwd: session.workspaceFolder?.uri.fsPath || process.cwd()
            }
        );

        console.log(`[Boa Debug] Debug adapter descriptor created`);
        return descriptor;
    }

    /**
     * Find the boa-cli executable
     * @returns {string|null}
     */
    findBoaCli() {
        const fs = require('fs');
        
        // Try to find boa-cli in the workspace (for development)
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (workspaceFolders && workspaceFolders.length > 0) {
            const workspaceRoot = workspaceFolders[0].uri.fsPath;
            
            // First, try to find the Boa repository root by looking for Cargo.toml with boa_cli
            const boaRepoRoot = this.findBoaRepositoryRoot(workspaceRoot);
            
            if (boaRepoRoot) {
                console.log(`[Boa Debug] Found Boa repository at: ${boaRepoRoot}`);
                
                // Check debug build first
                let cliPath = path.join(boaRepoRoot, 'target', 'debug', 'boa');
                if (process.platform === 'win32') {
                    cliPath += '.exe';
                }
                
                console.log(`[Boa Debug] Checking: ${cliPath}`);
                if (fs.existsSync(cliPath)) {
                    console.log(`[Boa Debug] Found boa-cli at: ${cliPath}`);
                    return cliPath;
                }

                // Check release build
                cliPath = path.join(boaRepoRoot, 'target', 'release', 'boa');
                if (process.platform === 'win32') {
                    cliPath += '.exe';
                }
                
                console.log(`[Boa Debug] Checking: ${cliPath}`);
                if (fs.existsSync(cliPath)) {
                    console.log(`[Boa Debug] Found boa-cli at: ${cliPath}`);
                    return cliPath;
                }
            } else {
                console.log(`[Boa Debug] Could not find Boa repository root from: ${workspaceRoot}`);
            }
        }

        // Fallback to PATH
        console.log('[Boa Debug] boa-cli not found in workspace, trying PATH');
        return 'boa';
    }

    /**
     * Find the Boa repository root by searching up the directory tree
     * @param {string} startPath - The path to start searching from
     * @returns {string|null} - The path to the Boa repository root, or null if not found
     */
    findBoaRepositoryRoot(startPath) {
        const fs = require('fs');
        let currentPath = startPath;
        
        // Search up the directory tree (max 10 levels to avoid infinite loop)
        for (let i = 0; i < 10; i++) {
            // Check if this directory has the Boa markers
            const cargoTomlPath = path.join(currentPath, 'Cargo.toml');
            const cliDirPath = path.join(currentPath, 'cli');
            
            console.log(`[Boa Debug] Checking for Boa repo at: ${currentPath}`);
            
            if (fs.existsSync(cargoTomlPath) && fs.existsSync(cliDirPath)) {
                // Verify it's actually the Boa repository by checking Cargo.toml content
                try {
                    const cargoContent = fs.readFileSync(cargoTomlPath, 'utf8');
                    if (cargoContent.includes('boa_cli') || cargoContent.includes('boa_engine')) {
                        console.log(`[Boa Debug] ✓ Found Boa repository root at: ${currentPath}`);
                        return currentPath;
                    }
                } catch (e) {
                    console.log(`[Boa Debug] Error reading Cargo.toml: ${e.message}`);
                }
            }
            
            // Move up one directory
            const parentPath = path.dirname(currentPath);
            
            // If we've reached the root, stop
            if (parentPath === currentPath) {
                break;
            }

            currentPath = parentPath;
        }
        
        return null;
    }
}

/**
 * Configuration provider for resolving debug configurations
 */
class BoaConfigurationProvider {
    /**
     * @param {vscode.DebugConfiguration} config
     * @param {vscode.CancellationToken} token
     * @returns {vscode.ProviderResult<vscode.DebugConfiguration>}
     */
    resolveDebugConfiguration(folder, config, token) {
        console.log(`[Boa Debug] Resolving debug configuration:`, config);

        // If no configuration is provided, create a default one
        if (!config.type && !config.request && !config.name) {
            const editor = vscode.window.activeTextEditor;
            if (editor && editor.document.languageId === 'javascript') {
                config.type = 'boa';
                config.name = 'Debug Current File';
                config.request = 'launch';
                config.program = editor.document.fileName;
                config.stopOnEntry = false;
                console.log(`[Boa Debug] Created default config for: ${config.program}`);
            }
        }

        // Ensure required fields are set
        if (!config.program) {
            const errorMsg = 'Cannot debug: No program specified in launch configuration.';
            console.error(`[Boa Debug] ${errorMsg}`);
            vscode.window.showErrorMessage(errorMsg);
            return null;
        }

        // Ensure cwd is set
        if (!config.cwd && folder) {
            config.cwd = folder.uri.fsPath;
        }

        console.log(`[Boa Debug] Final configuration:`, config);
        return config;
    }
}

module.exports = {
    activate,
    deactivate
};
