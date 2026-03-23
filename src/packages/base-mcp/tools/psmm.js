/**
 * BASE PSMM — Per-Session Meta Memory tools
 * Tracks significant meta moments across sessions.
 * The injection hook (psmm-injector.py) re-injects entries into context every prompt.
 * CARL connects only for graduation: PSMM entries can be staged as CARL rule proposals.
 */

import { readFileSync, writeFileSync, existsSync } from 'fs';
import { join } from 'path';

const VALID_TYPES = ['DECISION', 'CORRECTION', 'SHIFT', 'INSIGHT', 'COMMITMENT'];

function debugLog(...args) {
    console.error('[BASE:psmm]', new Date().toISOString(), ...args);
}

function getPsmmPath(workspacePath) {
    return join(workspacePath, '.base', 'data', 'psmm.json');
}

function readPsmm(workspacePath) {
    const filepath = getPsmmPath(workspacePath);
    if (!existsSync(filepath)) {
        return { sessions: {} };
    }
    try {
        return JSON.parse(readFileSync(filepath, 'utf-8'));
    } catch (error) {
        debugLog('Error reading psmm.json:', error.message);
        return { sessions: {} };
    }
}

function writePsmm(workspacePath, data) {
    const filepath = getPsmmPath(workspacePath);
    writeFileSync(filepath, JSON.stringify(data, null, 2), 'utf-8');
}

function formatTimestamp() {
    const now = new Date();
    const pad = (n) => String(n).padStart(2, '0');
    return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}`;
}

// ============================================================
// TOOL DEFINITIONS
// ============================================================

export const TOOLS = [
    {
        name: "base_psmm_log",
        description: "Log a per-session meta memory entry. Types: DECISION, CORRECTION, SHIFT, INSIGHT, COMMITMENT. Auto-creates session if new.",
        inputSchema: {
            type: "object",
            properties: {
                session_id: { type: "string", description: "Session UUID" },
                type: { type: "string", enum: VALID_TYPES, description: "Entry type" },
                text: { type: "string", description: "Description of the meta moment" }
            },
            required: ["session_id", "type", "text"]
        }
    },
    {
        name: "base_psmm_get",
        description: "Get all PSMM entries for a specific session by UUID.",
        inputSchema: {
            type: "object",
            properties: {
                session_id: { type: "string", description: "Session UUID" }
            },
            required: ["session_id"]
        }
    },
    {
        name: "base_psmm_list",
        description: "List all PSMM sessions with entry counts and created timestamps.",
        inputSchema: { type: "object", properties: {} }
    },
    {
        name: "base_psmm_clean",
        description: "Remove a stale session's entries from PSMM.",
        inputSchema: {
            type: "object",
            properties: {
                session_id: { type: "string", description: "Session UUID to remove" }
            },
            required: ["session_id"]
        }
    }
];

// ============================================================
// TOOL HANDLERS
// ============================================================

export async function handleTool(name, args, workspacePath) {
    switch (name) {
        case "base_psmm_log": return psmmLog(args, workspacePath);
        case "base_psmm_get": return psmmGet(args, workspacePath);
        case "base_psmm_list": return psmmList(workspacePath);
        case "base_psmm_clean": return psmmClean(args, workspacePath);
        default: return null;
    }
}

async function psmmLog(args, workspacePath) {
    const { session_id, type, text } = args;

    if (!VALID_TYPES.includes(type)) {
        return { success: false, error: `Invalid type: ${type}. Valid: ${VALID_TYPES.join(', ')}` };
    }

    debugLog('Logging PSMM entry:', session_id, type);

    const data = readPsmm(workspacePath);

    if (!data.sessions[session_id]) {
        data.sessions[session_id] = {
            created: formatTimestamp(),
            entries: []
        };
    }

    const entry = {
        timestamp: formatTimestamp(),
        type,
        text
    };

    data.sessions[session_id].entries.push(entry);
    writePsmm(workspacePath, data);

    return {
        success: true,
        session_id,
        entry_count: data.sessions[session_id].entries.length,
        message: `Logged ${type} entry to session ${session_id.slice(0, 8)}...`
    };
}

async function psmmGet(args, workspacePath) {
    const { session_id } = args;
    debugLog('Getting PSMM for session:', session_id);

    const data = readPsmm(workspacePath);
    const session = data.sessions[session_id];

    if (!session) {
        return { entries: [], exists: false };
    }

    return {
        exists: true,
        session_id,
        created: session.created,
        entry_count: session.entries.length,
        entries: session.entries
    };
}

async function psmmList(workspacePath) {
    debugLog('Listing PSMM sessions');

    const data = readPsmm(workspacePath);
    const sessions = [];

    for (const [id, session] of Object.entries(data.sessions)) {
        sessions.push({
            session_id: id,
            created: session.created,
            entry_count: session.entries.length,
            types: [...new Set(session.entries.map(e => e.type))]
        });
    }

    sessions.sort((a, b) => b.created.localeCompare(a.created));

    return {
        success: true,
        session_count: sessions.length,
        total_entries: sessions.reduce((sum, s) => sum + s.entry_count, 0),
        sessions
    };
}

async function psmmClean(args, workspacePath) {
    const { session_id } = args;
    debugLog('Cleaning PSMM session:', session_id);

    const data = readPsmm(workspacePath);

    if (!data.sessions[session_id]) {
        return { success: false, error: `Session not found: ${session_id}` };
    }

    const entryCount = data.sessions[session_id].entries.length;
    delete data.sessions[session_id];
    writePsmm(workspacePath, data);

    return {
        success: true,
        session_id,
        entries_removed: entryCount,
        message: `Cleaned session ${session_id.slice(0, 8)}... (${entryCount} entries removed)`
    };
}
