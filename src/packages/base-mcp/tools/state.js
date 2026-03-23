/**
 * APEX State — Read/update tools for state.json
 * Workspace health, drift tracking, groom scheduling
 */

import { readFileSync, writeFileSync, existsSync } from 'fs';
import { join } from 'path';

function debugLog(...args) {
    console.error('[BASE:state]', new Date().toISOString(), ...args);
}

// ============================================================
// HELPERS
// ============================================================

function getStatePath(workspacePath) {
    return join(workspacePath, '.base', 'data', 'state.json');
}

function readState(workspacePath) {
    const filepath = getStatePath(workspacePath);
    if (!existsSync(filepath)) {
        return { version: 1, workspace: '', last_modified: null, groom: {}, drift: { score: 0, indicators: {} }, areas: {}, satellites: {} };
    }
    try {
        return JSON.parse(readFileSync(filepath, 'utf-8'));
    } catch (error) {
        debugLog('Error reading state.json:', error.message);
        return { version: 1, workspace: '', last_modified: null, groom: {}, drift: { score: 0, indicators: {} }, areas: {}, satellites: {} };
    }
}

function writeState(workspacePath, data) {
    const filepath = getStatePath(workspacePath);
    data.last_modified = new Date().toISOString();
    writeFileSync(filepath, JSON.stringify(data, null, 2), 'utf-8');
}

function addDays(dateStr, days) {
    const d = new Date(dateStr);
    d.setDate(d.getDate() + days);
    return d.toISOString().split('T')[0];
}

function todayStr() {
    return new Date().toISOString().split('T')[0];
}

// ============================================================
// TOOL DEFINITIONS
// ============================================================

export const TOOLS = [
    {
        name: "apex_get_state",
        description: "Read full workspace state (groom, drift, areas, satellites, carl_hygiene). Returns entire state.json.",
        inputSchema: {
            type: "object",
            properties: {},
            required: []
        }
    },
    {
        name: "apex_update_drift",
        description: "Update drift indicators and recalculate composite score. Pass indicator key-value pairs to merge.",
        inputSchema: {
            type: "object",
            properties: {
                indicators: {
                    type: "object",
                    description: "Drift indicator updates (e.g., { active_age_days: 2, backlog_past_review: 3 })"
                }
            },
            required: ["indicators"]
        }
    },
    {
        name: "apex_record_groom",
        description: "Record a groom event. Sets last_groom to today and advances next_groom_due based on cadence.",
        inputSchema: {
            type: "object",
            properties: {},
            required: []
        }
    },
    {
        name: "apex_update_area",
        description: "Update a specific workspace area's fields (status, last_touched, groom_due, etc.).",
        inputSchema: {
            type: "object",
            properties: {
                area: { type: "string", description: "Area slug (key in state.json areas object)" },
                data: { type: "object", description: "Fields to merge into the area object" }
            },
            required: ["area", "data"]
        }
    }
];

// ============================================================
// TOOL HANDLERS
// ============================================================

function handleGetState(workspacePath) {
    debugLog('Reading state');
    return readState(workspacePath);
}

function handleUpdateDrift(args, workspacePath) {
    const { indicators } = args;
    if (!indicators) throw new Error('Missing required parameter: indicators');

    debugLog('Updating drift indicators');
    const data = readState(workspacePath);

    if (!data.drift) data.drift = { score: 0, indicators: {} };
    if (!data.drift.indicators) data.drift.indicators = {};

    // Merge indicators
    data.drift.indicators = { ...data.drift.indicators, ...indicators };

    // Recalculate score as sum of all indicator values
    data.drift.score = Object.values(data.drift.indicators)
        .reduce((sum, val) => sum + (typeof val === 'number' ? val : 0), 0);

    writeState(workspacePath, data);

    return {
        score: data.drift.score,
        indicators: data.drift.indicators
    };
}

function handleRecordGroom(workspacePath) {
    debugLog('Recording groom event');
    const data = readState(workspacePath);

    if (!data.groom) data.groom = { cadence: 'weekly', day: 'friday' };

    const today = todayStr();
    data.groom.last_groom = today;

    // Calculate next due based on cadence
    const cadenceDays = {
        daily: 1,
        weekly: 7,
        'bi-weekly': 14,
        monthly: 30
    };
    const days = cadenceDays[data.groom.cadence] || 7;
    data.groom.next_groom_due = addDays(today, days);

    writeState(workspacePath, data);

    return {
        last_groom: data.groom.last_groom,
        next_groom_due: data.groom.next_groom_due,
        cadence: data.groom.cadence
    };
}

function handleUpdateArea(args, workspacePath) {
    const { area, data: updateData } = args;
    if (!area) throw new Error('Missing required parameter: area');
    if (!updateData) throw new Error('Missing required parameter: data');

    debugLog('Updating area:', area);
    const data = readState(workspacePath);

    if (!data.areas) data.areas = {};
    if (!data.areas[area]) {
        throw new Error(`Area "${area}" not found. Available: ${Object.keys(data.areas).join(', ') || 'none'}`);
    }

    data.areas[area] = { ...data.areas[area], ...updateData };
    writeState(workspacePath, data);

    return data.areas[area];
}

// ============================================================
// HANDLER DISPATCH
// ============================================================

export function handleTool(name, args, workspacePath) {
    switch (name) {
        case 'apex_get_state':
            return handleGetState(workspacePath);
        case 'apex_update_drift':
            return handleUpdateDrift(args, workspacePath);
        case 'apex_record_groom':
            return handleRecordGroom(workspacePath);
        case 'apex_update_area':
            return handleUpdateArea(args, workspacePath);
        default:
            return null;
    }
}
