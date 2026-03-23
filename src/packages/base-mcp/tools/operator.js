/**
 * Operator CRUD tools for .base/operator.json
 * Read, update sections, toggle hook activation
 */

import fs from 'fs';
import path from 'path';

function debugLog(...args) {
    console.error('[BASE:operator]', new Date().toISOString(), ...args);
}

function getOperatorPath(workspacePath) {
    return path.join(workspacePath, '.base', 'operator.json');
}

function readOperator(workspacePath) {
    const filepath = getOperatorPath(workspacePath);
    try {
        return JSON.parse(fs.readFileSync(filepath, 'utf8'));
    } catch (error) {
        return null;
    }
}

function writeOperator(workspacePath, data) {
    const filepath = getOperatorPath(workspacePath);
    data.last_updated = new Date().toISOString();
    fs.writeFileSync(filepath, JSON.stringify(data, null, 2) + '\n');
}

// ============================================================
// TOOL DEFINITIONS
// ============================================================

export const TOOLS = [
    {
        name: "apex_get_operator",
        description: "Read the full operator profile from .base/operator.json. Returns identity, deep why, north star, values, pitch, vision.",
        inputSchema: { type: "object", properties: {}, required: [] }
    },
    {
        name: "apex_update_operator",
        description: "Update a specific section of the operator profile. Sections: deep_why, north_star, key_values, elevator_pitch, surface_vision, extensions, hook_active. Pass the full section object to replace it.",
        inputSchema: {
            type: "object",
            properties: {
                section: { type: "string", description: "Section to update (deep_why, north_star, key_values, elevator_pitch, surface_vision, extensions, hook_active)" },
                data: { description: "New section data (object for sections, boolean for hook_active)" }
            },
            required: ["section", "data"]
        }
    }
];

// ============================================================
// TOOL HANDLERS
// ============================================================

async function handleGetOperator(_args, workspacePath) {
    const data = readOperator(workspacePath);
    if (!data) {
        return { error: "No operator.json found. Run /base:orientation to create one." };
    }
    return data;
}

async function handleUpdateOperator(args, workspacePath) {
    const { section, data: sectionData } = args;
    if (!section) throw new Error('Missing required parameter: section');
    if (sectionData === undefined) throw new Error('Missing required parameter: data');

    const validSections = ['deep_why', 'north_star', 'key_values', 'elevator_pitch', 'surface_vision', 'extensions', 'hook_active'];
    if (!validSections.includes(section)) {
        throw new Error(`Invalid section "${section}". Valid: ${validSections.join(', ')}`);
    }

    const operator = readOperator(workspacePath);
    if (!operator) {
        throw new Error('No operator.json found. Run /base:orientation to create one.');
    }

    debugLog('Updating operator section:', section);

    if (section === 'hook_active') {
        operator.hook_active = !!sectionData;
    } else {
        operator[section] = sectionData;
    }

    writeOperator(workspacePath, operator);

    return {
        section,
        updated: true,
        hook_active: operator.hook_active
    };
}

export async function handleTool(name, args, workspacePath) {
    switch (name) {
        case 'apex_get_operator': return handleGetOperator(args, workspacePath);
        case 'apex_update_operator': return handleUpdateOperator(args, workspacePath);
        default: return null;
    }
}
