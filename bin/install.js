#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const os = require('os');
const readline = require('readline');
const { execSync } = require('child_process');

// Colors
const cyan = '\x1b[36m';
const green = '\x1b[32m';
const yellow = '\x1b[33m';
const dim = '\x1b[2m';
const reset = '\x1b[0m';

// Get version from package.json
const pkg = require('../package.json');

const banner = `
${cyan}  ██████╗  █████╗ ███████╗███████╗
  ██╔══██╗██╔══██╗██╔════╝██╔════╝
  ██████╔╝███████║███████╗█████╗
  ██╔══██╗██╔══██║╚════██║██╔══╝
  ██████╔╝██║  ██║███████║███████╗
  ╚═════╝ ╚═╝  ╚═╝╚══════╝╚══════╝${reset}

  BASE Framework ${dim}v${pkg.version}${reset}
  Builder's Automated State Engine for Claude Code
`;

// Parse args
const args = process.argv.slice(2);
const hasGlobal = args.includes('--global') || args.includes('-g');
const hasLocal = args.includes('--local') || args.includes('-l');
const hasWorkspace = args.includes('--workspace') || args.includes('-w');

// Parse --config-dir argument
function parseConfigDirArg() {
  const configDirIndex = args.findIndex(arg => arg === '--config-dir' || arg === '-c');
  if (configDirIndex !== -1) {
    const nextArg = args[configDirIndex + 1];
    if (!nextArg || nextArg.startsWith('-')) {
      console.error(`  ${yellow}--config-dir requires a path argument${reset}`);
      process.exit(1);
    }
    return nextArg;
  }
  const configDirArg = args.find(arg => arg.startsWith('--config-dir=') || arg.startsWith('-c='));
  if (configDirArg) {
    return configDirArg.split('=')[1];
  }
  return null;
}
const explicitConfigDir = parseConfigDirArg();

// Parse --workspace-dir argument
function parseWorkspaceDirArg() {
  const idx = args.findIndex(arg => arg === '--workspace-dir');
  if (idx !== -1) {
    const nextArg = args[idx + 1];
    if (!nextArg || nextArg.startsWith('-')) {
      console.error(`  ${yellow}--workspace-dir requires a path argument${reset}`);
      process.exit(1);
    }
    return nextArg;
  }
  return null;
}
const explicitWorkspaceDir = parseWorkspaceDirArg();
const hasHelp = args.includes('--help') || args.includes('-h');

console.log(banner);

// Show help if requested
if (hasHelp) {
  console.log(`  ${yellow}Usage:${reset} npx base-framework [options]

  ${yellow}Options:${reset}
    ${cyan}-g, --global${reset}                 Install commands globally (to ~/.claude)
    ${cyan}-l, --local${reset}                  Install commands locally (to ./.claude)
    ${cyan}-w, --workspace${reset}              Install workspace layer (.base/ in current directory)
    ${cyan}-c, --config-dir <path>${reset}      Specify custom Claude config directory
    ${cyan}--workspace-dir <path>${reset}        Specify workspace root (default: cwd)
    ${cyan}-h, --help${reset}                   Show this help message

  ${yellow}Examples:${reset}
    ${dim}# Full install: global commands + workspace layer${reset}
    npx base-framework --global --workspace

    ${dim}# Global commands only (no workspace data)${reset}
    npx base-framework --global

    ${dim}# Workspace layer only (assumes commands already installed)${reset}
    npx base-framework --workspace

    ${dim}# Install to current project only${reset}
    npx base-framework --local --workspace

  ${yellow}What gets installed:${reset}
    ${cyan}Commands (--global or --local):${reset}
      commands/base/       - Slash commands (/base:surface-create, etc.)
      skills/base/         - Skill framework (tasks, templates, context)

    ${cyan}Workspace (--workspace):${reset}
      .base/data/          - JSON data surfaces (active, backlog, projects, entities, state)
      .base/hooks/         - All hooks (surface + session + command)
      .base/base-mcp/      - BASE MCP server (npm install auto-runs)
      .base/workspace.json - Workspace manifest
      .base/operator.json  - Operator profile (guided setup via /base:scaffold)
      .mcp.json            - MCP server registration (merged)
`);
  process.exit(0);
}

/**
 * Expand ~ to home directory
 */
function expandTilde(filePath) {
  if (filePath && filePath.startsWith('~/')) {
    return path.join(os.homedir(), filePath.slice(2));
  }
  return filePath;
}

/**
 * Recursively copy directory
 */
function copyDir(srcDir, destDir) {
  fs.mkdirSync(destDir, { recursive: true });
  const entries = fs.readdirSync(srcDir, { withFileTypes: true });
  for (const entry of entries) {
    const srcPath = path.join(srcDir, entry.name);
    const destPath = path.join(destDir, entry.name);
    if (entry.isDirectory()) {
      copyDir(srcPath, destPath);
    } else {
      fs.copyFileSync(srcPath, destPath);
    }
  }
}

/**
 * Write JSON file only if it doesn't already exist
 */
function writeJsonIfNew(filePath, data) {
  if (!fs.existsSync(filePath)) {
    fs.writeFileSync(filePath, JSON.stringify(data, null, 2));
    return true;
  }
  return false;
}

/**
 * Detect v1/v2 artifacts that need migration.
 * Returns array of { path, label, type } objects.
 * Does NOT modify anything — detection only.
 */
function detectV1Artifacts(claudeDir, workspaceDir) {
  const found = [];

  if (workspaceDir) {
    // carl-mcp in .base/ (no longer ships with BASE)
    const carlMcpDir = path.join(workspaceDir, '.base', 'carl-mcp');
    if (fs.existsSync(carlMcpDir)) {
      found.push({
        path: carlMcpDir,
        label: '.base/carl-mcp/',
        reason: 'CARL MCP no longer ships with BASE — install carl-core separately',
        type: 'directory'
      });
    }

    // Session hooks in .claude/hooks/ that moved to .base/hooks/
    const movedHooks = ['base-pulse-check.py', 'psmm-injector.py', 'satellite-detection.py'];
    const claudeHooksDir = path.join(workspaceDir, '.claude', 'hooks');
    if (fs.existsSync(claudeHooksDir)) {
      for (const hook of movedHooks) {
        const hookPath = path.join(claudeHooksDir, hook);
        if (fs.existsSync(hookPath)) {
          found.push({
            path: hookPath,
            label: `.claude/hooks/${hook}`,
            reason: 'Hooks now live in .base/hooks/ — duplicate here causes double-fire',
            type: 'file'
          });
        }
      }
    }

    // carl-mcp entry in .mcp.json
    const mcpJsonPath = path.join(workspaceDir, '.mcp.json');
    if (fs.existsSync(mcpJsonPath)) {
      try {
        const mcpConfig = JSON.parse(fs.readFileSync(mcpJsonPath, 'utf-8'));
        if (mcpConfig.mcpServers && mcpConfig.mcpServers['carl-mcp']) {
          found.push({
            path: mcpJsonPath,
            label: '.mcp.json → carl-mcp entry',
            reason: 'CARL MCP no longer registered by BASE — install carl-core for CARL MCP',
            type: 'mcp-entry'
          });
        }
      } catch (e) { /* skip */ }
    }
  }

  // Old markdown templates in base-framework/
  if (claudeDir) {
    const oldTemplates = ['active-md.md', 'backlog-md.md', 'state-md.md'];
    const templatesDir = path.join(claudeDir, 'base-framework', 'templates');
    if (fs.existsSync(templatesDir)) {
      for (const tmpl of oldTemplates) {
        const tmplPath = path.join(templatesDir, tmpl);
        if (fs.existsSync(tmplPath)) {
          found.push({
            path: tmplPath,
            label: `base-framework/templates/${tmpl}`,
            reason: 'Replaced by JSON templates in v3 — no longer used by scaffold',
            type: 'file'
          });
        }
      }
    }
  }

  return found;
}

/**
 * Archive a single artifact (file or directory) to .base/_archive/upgrade-v3/
 */
function archiveArtifact(artifact, archiveDir) {
  fs.mkdirSync(archiveDir, { recursive: true });

  if (artifact.type === 'directory') {
    const destName = path.basename(artifact.path);
    const destPath = path.join(archiveDir, destName);
    copyDir(artifact.path, destPath);
    fs.rmSync(artifact.path, { recursive: true });
  } else if (artifact.type === 'file') {
    const destPath = path.join(archiveDir, path.basename(artifact.path));
    fs.copyFileSync(artifact.path, destPath);
    fs.unlinkSync(artifact.path);
  } else if (artifact.type === 'mcp-entry') {
    // For MCP entries, remove the key from JSON
    const mcpConfig = JSON.parse(fs.readFileSync(artifact.path, 'utf-8'));
    delete mcpConfig.mcpServers['carl-mcp'];
    fs.writeFileSync(artifact.path, JSON.stringify(mcpConfig, null, 2));
  }
}

/**
 * Run upgrade cleanup interactively.
 * Detects v1/v2 artifacts, warns the user, prompts for confirmation, archives.
 * Returns a promise that resolves when cleanup is complete or skipped.
 */
function runUpgradeCleanup(claudeDir, workspaceDir) {
  const artifacts = detectV1Artifacts(claudeDir, workspaceDir);
  if (artifacts.length === 0) return Promise.resolve();

  const baseDir = workspaceDir ? path.join(workspaceDir, '.base') : path.join(process.cwd(), '.base');
  const archiveDir = path.join(baseDir, '_archive', 'upgrade-v3');

  console.log(`  ${yellow}=== UPGRADE DETECTED ===${reset}`);
  console.log(`  Found ${artifacts.length} artifact(s) from a previous BASE version:\n`);
  for (const a of artifacts) {
    console.log(`  ${yellow}!${reset} ${a.label}`);
    console.log(`    ${dim}${a.reason}${reset}`);
  }
  console.log(`\n  ${dim}These will be archived to: ${archiveDir.replace(os.homedir(), '~')}${reset}`);
  console.log(`  ${dim}Nothing is deleted — you can recover from the archive.${reset}\n`);

  return new Promise((resolve) => {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    rl.question(`  Archive these artifacts? ${dim}[Y/n]${reset}: `, (answer) => {
      rl.close();
      const proceed = !answer.trim() || answer.trim().toLowerCase() === 'y';
      if (proceed) {
        for (const a of artifacts) {
          archiveArtifact(a, archiveDir);
          console.log(`  ${green}archived${reset} ${a.label}`);
        }
        console.log('');
      } else {
        console.log(`  ${dim}Skipped cleanup. Old artifacts left in place.${reset}\n`);
      }
      resolve();
    });
  });
}

/**
 * Install commands and skill framework
 */
function installCommands(isGlobal) {
  const src = path.join(__dirname, '..');
  const configDir = expandTilde(explicitConfigDir) || expandTilde(process.env.CLAUDE_CONFIG_DIR);
  const defaultGlobalDir = configDir || path.join(os.homedir(), '.claude');
  const claudeDir = isGlobal ? defaultGlobalDir : path.join(process.cwd(), '.claude');

  const locationLabel = isGlobal
    ? claudeDir.replace(os.homedir(), '~')
    : claudeDir.replace(process.cwd(), '.');

  console.log(`  Installing commands to ${cyan}${locationLabel}${reset}\n`);

  // Copy commands
  const commandsSrc = path.join(src, 'src', 'commands');
  const commandsDest = path.join(claudeDir, 'commands', 'base');
  copyDir(commandsSrc, commandsDest);
  const commandCount = fs.readdirSync(commandsSrc).filter(f => f.endsWith('.md')).length;
  console.log(`  ${green}+${reset} commands/base/ (${commandCount} slash commands)`);

  // Copy skill entry point
  const skillSrc = path.join(src, 'src', 'skill');
  const skillDest = path.join(claudeDir, 'skills', 'base');
  copyDir(skillSrc, skillDest);
  console.log(`  ${green}+${reset} skills/base/ (entry point + MCP package sources)`);

  // Copy MCP package sources into skill (for scaffold reference)
  const packagesSrc = path.join(src, 'src', 'packages');
  const packagesDest = path.join(claudeDir, 'skills', 'base', 'packages');
  copyDir(packagesSrc, packagesDest);

  // Copy BASE framework (tasks, templates, context, frameworks)
  const frameworkSrc = path.join(src, 'src', 'framework');
  const frameworkDest = path.join(claudeDir, 'base-framework');
  copyDir(frameworkSrc, frameworkDest);
  console.log(`  ${green}+${reset} base-framework/ (tasks, templates, context, frameworks, utils)`);

  // Copy all hooks to base-framework/hooks/ (source for scaffold)
  const hooksFrameworkDest = path.join(claudeDir, 'base-framework', 'hooks');
  fs.mkdirSync(hooksFrameworkDest, { recursive: true });
  const hooksSrcDir = path.join(src, 'src', 'hooks');
  const hookFiles = fs.readdirSync(hooksSrcDir).filter(f => f.endsWith('.py'));
  for (const hookFile of hookFiles) {
    fs.copyFileSync(path.join(hooksSrcDir, hookFile), path.join(hooksFrameworkDest, hookFile));
  }
  console.log(`  ${green}+${reset} base-framework/hooks/ (${hookFiles.length} hooks for scaffold)`);

  console.log(`\n  ${green}Commands installed.${reset}\n`);
}

/**
 * Install workspace layer (.base/)
 */
function installWorkspace() {
  const src = path.join(__dirname, '..');
  const workspaceDir = expandTilde(explicitWorkspaceDir) || process.cwd();
  const baseDir = path.join(workspaceDir, '.base');

  console.log(`  Installing workspace layer to ${cyan}${baseDir.replace(os.homedir(), '~')}${reset}\n`);

  // Create .base directories
  fs.mkdirSync(path.join(baseDir, 'data'), { recursive: true });
  fs.mkdirSync(path.join(baseDir, 'hooks'), { recursive: true });
  console.log(`  ${green}+${reset} .base/data/`);
  console.log(`  ${green}+${reset} .base/hooks/`);

  // Initialize JSON data surfaces (don't overwrite existing)
  const dataSurfaces = {
    'active.json': { items: [], last_updated: null },
    'backlog.json': { items: [], last_updated: null },
    'projects.json': { projects: [], last_updated: null },
    'entities.json': { entities: [], last_updated: null },
    'state.json': { drift_score: 0, areas: {}, last_groom: null, last_updated: null },
    'psmm.json': { sessions: {} }
  };
  let surfaceCount = 0;
  for (const [filename, data] of Object.entries(dataSurfaces)) {
    if (writeJsonIfNew(path.join(baseDir, 'data', filename), data)) {
      surfaceCount++;
    }
  }
  if (surfaceCount > 0) {
    console.log(`  ${green}+${reset} .base/data/ (${surfaceCount} JSON surfaces initialized)`);
  } else {
    console.log(`  ${dim}  .base/data/ (existing surfaces preserved)${reset}`);
  }

  // Copy workspace.json template (don't overwrite existing)
  const workspaceJsonDest = path.join(baseDir, 'workspace.json');
  if (!fs.existsSync(workspaceJsonDest)) {
    const templateSrc = path.join(src, 'src', 'templates', 'workspace.json');
    if (fs.existsSync(templateSrc)) {
      const template = JSON.parse(fs.readFileSync(templateSrc, 'utf-8'));
      template.workspace = path.basename(workspaceDir);
      template.created = new Date().toISOString().split('T')[0];
      fs.writeFileSync(workspaceJsonDest, JSON.stringify(template, null, 2));
      console.log(`  ${green}+${reset} .base/workspace.json (manifest)`);
    }
  } else {
    console.log(`  ${dim}  .base/workspace.json (existing manifest preserved)${reset}`);
  }

  // Copy operator.json template (don't overwrite existing)
  const operatorJsonDest = path.join(baseDir, 'operator.json');
  if (!fs.existsSync(operatorJsonDest)) {
    const operatorSrc = path.join(src, 'src', 'templates', 'operator.json');
    if (fs.existsSync(operatorSrc)) {
      fs.copyFileSync(operatorSrc, operatorJsonDest);
      console.log(`  ${green}+${reset} .base/operator.json (operator profile — complete via /base:scaffold)`);
    }
  } else {
    console.log(`  ${dim}  .base/operator.json (existing profile preserved)${reset}`);
  }

  // Copy base-mcp
  const baseMcpSrc = path.join(src, 'src', 'packages', 'base-mcp');
  const baseMcpDest = path.join(baseDir, 'base-mcp');
  copyDir(baseMcpSrc, baseMcpDest);
  console.log(`  ${green}+${reset} .base/base-mcp/`);

  // Copy all hooks to .base/hooks/
  const allHooksSrc = path.join(src, 'src', 'hooks');
  const hookEntries = fs.readdirSync(allHooksSrc).filter(f => f.endsWith('.py'));
  for (const file of hookEntries) {
    fs.copyFileSync(path.join(allHooksSrc, file), path.join(baseDir, 'hooks', file));
  }
  console.log(`  ${green}+${reset} .base/hooks/ (${hookEntries.length} hooks)`);

  // npm install for base-mcp
  console.log(`\n  Installing MCP dependencies...`);
  try {
    execSync('npm install', { cwd: baseMcpDest, stdio: 'pipe' });
    console.log(`  ${green}+${reset} base-mcp dependencies installed`);
  } catch (e) {
    console.log(`  ${yellow}!${reset} base-mcp npm install failed — run manually in .base/base-mcp/`);
  }

  // Merge MCP registration into .mcp.json
  const mcpJsonPath = path.join(workspaceDir, '.mcp.json');
  let mcpConfig = { mcpServers: {} };
  if (fs.existsSync(mcpJsonPath)) {
    try {
      mcpConfig = JSON.parse(fs.readFileSync(mcpJsonPath, 'utf-8'));
    } catch (e) { /* start fresh */ }
  }
  mcpConfig.mcpServers['base-mcp'] = {
    type: 'stdio',
    command: 'node',
    args: ['./.base/base-mcp/index.js']
  };
  fs.writeFileSync(mcpJsonPath, JSON.stringify(mcpConfig, null, 2));
  console.log(`  ${green}+${reset} .mcp.json (base-mcp registered)`);

  console.log(`\n  ${green}Workspace layer installed.${reset}`);
  console.log(`  Run ${cyan}/base:scaffold${reset} to complete setup (hook wiring, operator profile).\n`);
}

/**
 * Prompt for install location
 */
function promptLocation() {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout
  });

  const configDir = expandTilde(explicitConfigDir) || expandTilde(process.env.CLAUDE_CONFIG_DIR);
  const globalPath = configDir || path.join(os.homedir(), '.claude');
  const globalLabel = globalPath.replace(os.homedir(), '~');

  console.log(`  ${yellow}What would you like to install?${reset}

  ${cyan}1${reset}) Full install  ${dim}(commands to ${globalLabel} + workspace layer to .base/)${reset}
  ${cyan}2${reset}) Commands only  ${dim}(${globalLabel})${reset}
  ${cyan}3${reset}) Workspace only ${dim}(.base/ in current directory)${reset}
`);

  rl.question(`  Choice ${dim}[1]${reset}: `, (answer) => {
    rl.close();
    const choice = answer.trim() || '1';
    if (choice === '1' || choice === '2') {
      installCommands(true);
    }
    if (choice === '1' || choice === '3') {
      installWorkspace();
    }
    if (choice !== '1' && choice !== '2' && choice !== '3') {
      installCommands(true);
      installWorkspace();
    }
    console.log(`  ${green}Done!${reset} Launch Claude Code and run ${cyan}/base:scaffold${reset} to complete setup.\n`);
  });
}

// Main
async function main() {
  if (hasHelp) {
    return; // Already handled above
  }

  if (hasGlobal || hasLocal || hasWorkspace) {
    if (hasGlobal && hasLocal) {
      console.error(`  ${yellow}Cannot specify both --global and --local${reset}`);
      process.exit(1);
    }

    // Detect and offer cleanup of v1/v2 artifacts before installing
    const configDir = expandTilde(explicitConfigDir) || expandTilde(process.env.CLAUDE_CONFIG_DIR);
    const cleanupClaudeDir = (hasGlobal || hasLocal)
      ? (hasGlobal ? (configDir || path.join(os.homedir(), '.claude')) : path.join(process.cwd(), '.claude'))
      : null;
    const cleanupWorkspaceDir = hasWorkspace
      ? (expandTilde(explicitWorkspaceDir) || process.cwd())
      : null;

    await runUpgradeCleanup(cleanupClaudeDir, cleanupWorkspaceDir);

    if (hasGlobal || hasLocal) {
      installCommands(hasGlobal);
    }
    if (hasWorkspace) {
      installWorkspace();
    }
    console.log(`  ${green}Done!${reset} Launch Claude Code and run ${cyan}/base:scaffold${reset} to complete setup.\n`);
  } else {
    promptLocation();
  }
}

main();
