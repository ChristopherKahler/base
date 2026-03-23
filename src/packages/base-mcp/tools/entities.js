/**
 * APEX Entities — CRUD for entities.json
 * People and organizations with relational links to projects
 */

import { readFileSync, writeFileSync, existsSync } from 'fs';
import { join } from 'path';

function debugLog(...args) {
    console.error('[BASE:entities]', new Date().toISOString(), ...args);
}

// ============================================================
// HELPERS
// ============================================================

function getEntitiesPath(workspacePath) {
    return join(workspacePath, '.base', 'data', 'entities.json');
}

function readEntities(workspacePath) {
    const filepath = getEntitiesPath(workspacePath);
    if (!existsSync(filepath)) {
        return { version: 1, last_modified: null, entities: [] };
    }
    try {
        return JSON.parse(readFileSync(filepath, 'utf-8'));
    } catch (error) {
        debugLog('Error reading entities.json:', error.message);
        return { version: 1, last_modified: null, entities: [] };
    }
}

function writeEntities(workspacePath, data) {
    const filepath = getEntitiesPath(workspacePath);
    data.last_modified = new Date().toISOString();
    writeFileSync(filepath, JSON.stringify(data, null, 2), 'utf-8');
}

function generateEntityId(entities) {
    let max = 0;
    for (const entity of entities) {
        const match = (entity.id || '').match(/^ENT-(\d+)$/);
        if (match) {
            const num = parseInt(match[1], 10);
            if (num > max) max = num;
        }
    }
    return `ENT-${String(max + 1).padStart(3, '0')}`;
}

function formatTimestamp() {
    return new Date().toISOString();
}

// ============================================================
// TOOL DEFINITIONS
// ============================================================

export const TOOLS = [
    {
        name: "apex_list_entities",
        description: "List all entities with optional type filter (person/organization).",
        inputSchema: {
            type: "object",
            properties: {
                type: { type: "string", enum: ["person", "organization"], description: "Filter by entity type" }
            },
            required: []
        }
    },
    {
        name: "apex_add_entity",
        description: "Add a new person or organization entity. Auto-generates ENT-NNN ID.",
        inputSchema: {
            type: "object",
            properties: {
                name: { type: "string", description: "Display name" },
                type: { type: "string", enum: ["person", "organization"], description: "Entity type" },
                role: { type: "string", description: "Primary role (owner, client, partner, contractor, team member, employer)" }
            },
            required: ["name", "type"]
        }
    },
    {
        name: "apex_update_entity",
        description: "Update an entity's fields by ID. Shallow merge — only specified fields updated.",
        inputSchema: {
            type: "object",
            properties: {
                id: { type: "string", description: "Entity ID (e.g., 'ENT-001')" },
                data: { type: "object", description: "Fields to update" }
            },
            required: ["id", "data"]
        }
    },
    {
        name: "apex_link_entity",
        description: "Add a relation between an entity and a project item. Avoids duplicate relations.",
        inputSchema: {
            type: "object",
            properties: {
                id: { type: "string", description: "Entity ID (e.g., 'ENT-001')" },
                project_id: { type: "string", description: "Project item ID (e.g., 'PRJ-003')" },
                relationship: { type: "string", description: "Relationship type (stakeholder, client, owner, partner, contractor, assignee)" }
            },
            required: ["id", "project_id", "relationship"]
        }
    }
];

// ============================================================
// TOOL HANDLERS
// ============================================================

function handleListEntities(args, workspacePath) {
    debugLog('Listing entities');
    const data = readEntities(workspacePath);
    let entities = data.entities;

    if (args.type) {
        entities = entities.filter(e => e.type === args.type);
    }

    return { entities, count: entities.length, total: data.entities.length };
}

function handleAddEntity(args, workspacePath) {
    const { name, type } = args;
    if (!name) throw new Error('Missing required parameter: name');
    if (!type) throw new Error('Missing required parameter: type');

    const data = readEntities(workspacePath);
    const now = formatTimestamp();
    const id = generateEntityId(data.entities);

    const entity = {
        id,
        name,
        type,
        role: args.role || null,
        relations: [],
        notes: [],
        created_at: now,
        updated_at: now
    };

    data.entities.push(entity);
    writeEntities(workspacePath, data);

    debugLog(`Added ${type} ${id}: ${name}`);
    return entity;
}

function handleUpdateEntity(args, workspacePath) {
    const { id, data: updateData } = args;
    if (!id) throw new Error('Missing required parameter: id');
    if (!updateData) throw new Error('Missing required parameter: data');

    const data = readEntities(workspacePath);
    const index = data.entities.findIndex(e => e.id === id);
    if (index === -1) {
        throw new Error(`Entity "${id}" not found. Available: ${data.entities.map(e => e.id).join(', ') || 'none'}`);
    }

    data.entities[index] = {
        ...data.entities[index],
        ...updateData,
        id, // Prevent ID overwrite
        updated_at: formatTimestamp()
    };

    writeEntities(workspacePath, data);

    debugLog(`Updated ${id}`);
    return data.entities[index];
}

function handleLinkEntity(args, workspacePath) {
    const { id, project_id, relationship } = args;
    if (!id) throw new Error('Missing required parameter: id');
    if (!project_id) throw new Error('Missing required parameter: project_id');
    if (!relationship) throw new Error('Missing required parameter: relationship');

    const data = readEntities(workspacePath);
    const index = data.entities.findIndex(e => e.id === id);
    if (index === -1) {
        throw new Error(`Entity "${id}" not found`);
    }

    const entity = data.entities[index];
    if (!entity.relations) entity.relations = [];

    // Check for duplicate
    const exists = entity.relations.some(r => r.project_id === project_id && r.relationship === relationship);
    if (exists) {
        return { already_linked: true, id, project_id, relationship, message: 'Relation already exists' };
    }

    entity.relations.push({ project_id, relationship });
    entity.updated_at = formatTimestamp();

    writeEntities(workspacePath, data);

    debugLog(`Linked ${id} → ${project_id} (${relationship})`);
    return { id, project_id, relationship, relations_count: entity.relations.length };
}

// ============================================================
// HANDLER DISPATCH
// ============================================================

export function handleTool(name, args, workspacePath) {
    switch (name) {
        case 'apex_list_entities':
            return handleListEntities(args, workspacePath);
        case 'apex_add_entity':
            return handleAddEntity(args, workspacePath);
        case 'apex_update_entity':
            return handleUpdateEntity(args, workspacePath);
        case 'apex_link_entity':
            return handleLinkEntity(args, workspacePath);
        default:
            return null;
    }
}
