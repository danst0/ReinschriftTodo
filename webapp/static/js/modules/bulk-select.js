/**
 * Multi-select mode with bulk actions (issue #8).
 *
 * While select mode is active, clicks on todo items toggle selection
 * instead of completing/editing, swipe gestures are suppressed and
 * auto-reload is paused (a background partial reload would renumber the
 * line indexes the selection relies on).
 */

import { fetchWithCsrf } from './api.js';
import { stopAutoReload, startAutoReload } from './auto-reload.js';
import { showUndoToast } from './undo-toast.js';

let selectMode = false;
// line index (string, from data-line-index) -> marker (may be '' for
// hand-written lines). The marker is what survives the file being rewritten
// underneath the page; the index only describes how the page was rendered.
const selected = new Map();

let translations = {};
let onReload = null;
let autoReloadRestart = null;

/**
 * Initialize the bulk-select module.
 * @param {object} options
 * @param {object} options.translations - Localized strings
 * @param {function} options.reloadCallback - Reload after a bulk action
 * @param {function} options.autoReloadRestart - Re-arm auto-reload on exit
 */
export function initBulkSelect(options = {}) {
    translations = options.translations || {};
    onReload = options.reloadCallback || null;
    autoReloadRestart = options.autoReloadRestart || null;

    // Capture-phase listener: in select mode a click anywhere on a todo
    // item toggles its selection and never reaches the inline handlers
    // (checkbox toggle, edit modal, postpone buttons).
    document.addEventListener('click', (e) => {
        if (!selectMode) return;
        const item = e.target.closest('.todo-item');
        if (!item || !item.closest('.todo-list')) return;
        e.preventDefault();
        e.stopPropagation();
        toggleItemSelection(item);
    }, true);

    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && selectMode) {
            exitSelectMode();
        }
    });
}

/**
 * Whether select mode is currently active.
 * @returns {boolean}
 */
export function isSelectMode() {
    return selectMode;
}

/**
 * Toggle select mode on/off.
 */
export function toggleSelectMode() {
    if (selectMode) {
        exitSelectMode();
    } else {
        enterSelectMode();
    }
}

function enterSelectMode() {
    selectMode = true;
    document.body.classList.add('select-mode');
    document.getElementById('toggle-select')?.classList.add('active');
    document.getElementById('bulk-action-bar')?.removeAttribute('hidden');
    // Pause background reloads: they renumber line indexes.
    stopAutoReload();
    updateBar();
}

/**
 * Leave select mode and clear the selection.
 */
export function exitSelectMode() {
    selectMode = false;
    selected.clear();
    document.body.classList.remove('select-mode');
    document.getElementById('toggle-select')?.classList.remove('active');
    document.getElementById('bulk-action-bar')?.setAttribute('hidden', '');
    document.querySelectorAll('.todo-item.selected')
        .forEach(el => el.classList.remove('selected'));
    if (autoReloadRestart) autoReloadRestart();
}

function toggleItemSelection(item) {
    const lineIndex = item.dataset.lineIndex;
    if (lineIndex === undefined || lineIndex === '') return;
    if (selected.has(lineIndex)) {
        selected.delete(lineIndex);
        item.classList.remove('selected');
    } else {
        selected.set(lineIndex, item.dataset.marker || '');
        item.classList.add('selected');
    }
    updateBar();
}

/**
 * Re-apply selection classes after the todo list was re-rendered
 * (e.g. a manual partial reload while selecting).
 */
export function reattachSelectState() {
    if (!selectMode) return;
    const selectedMarkers = new Set([...selected.values()].filter(Boolean));
    document.querySelectorAll('.todo-item').forEach(item => {
        // Match on the marker where there is one: a re-render renumbers the
        // indexes, so re-applying by index would light up the wrong rows.
        const marker = item.dataset.marker || '';
        item.classList.toggle(
            'selected',
            marker ? selectedMarkers.has(marker) : selected.has(item.dataset.lineIndex)
        );
    });
    updateBar();
}

function updateBar() {
    const counter = document.getElementById('bulk-count');
    if (counter) {
        const template = translations.selectedCount || '{count}';
        counter.textContent = template.replace('{count}', String(selected.size));
    }
    document.querySelectorAll('#bulk-action-bar button[data-bulk-action]')
        .forEach(btn => { btn.disabled = selected.size === 0; });
}

function lineIndexes() {
    return [...selected.keys()].map(Number);
}

/** Markers of the selection, positionally matching lineIndexes(). */
function markers() {
    return [...selected.values()];
}

async function runBatch(url, payload) {
    const res = await fetchWithCsrf(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
    });
    if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
    }
    return res.json();
}

async function finishAction(message, updated) {
    exitSelectMode();
    if (onReload) await onReload();
    if (message) {
        showUndoToast(`${message} (${updated})`);
    }
}

/**
 * Complete or reopen all selected todos.
 * @param {boolean} done
 */
export async function bulkComplete(done) {
    if (!selected.size) return;
    try {
        const result = await runBatch('/api/toggle-batch', {
            line_indexes: lineIndexes(),
            markers: markers(),
            done
        });
        await finishAction(
            done ? translations.bulkComplete : translations.bulkReopen,
            result.updated
        );
    } catch (err) {
        console.error('Bulk toggle failed', err);
    }
}

/**
 * Set the due date of all selected todos.
 * @param {string} target - 'today' | 'tomorrow' | 'weekend' | 'sometime'
 */
export async function bulkSetDue(target) {
    if (!selected.size) return;
    try {
        const result = await runBatch('/api/postpone-batch', {
            line_indexes: lineIndexes(),
            markers: markers(),
            target
        });
        await finishAction(translations.bulkSetDue, result.updated);
    } catch (err) {
        console.error('Bulk postpone failed', err);
    }
}

/**
 * Delete all selected todos (after confirmation).
 */
export async function bulkDelete() {
    if (!selected.size) return;
    const template = translations.bulkDeleteConfirm || 'Delete {count} items?';
    if (!confirm(template.replace('{count}', String(selected.size)))) return;
    try {
        const result = await runBatch('/api/delete-batch', {
            line_indexes: lineIndexes(),
            markers: markers()
        });
        await finishAction(translations.bulkDelete, result.updated);
    } catch (err) {
        console.error('Bulk delete failed', err);
    }
}

/**
 * Open the assign dialog for the current selection.
 */
export function openBulkAssign() {
    if (!selected.size) return;
    const modal = document.getElementById('bulkAssignModal');
    if (!modal) return;
    modal.style.display = 'block';
    document.getElementById('bulk-assign-project')?.focus();
}

/**
 * Close the assign dialog.
 */
export function closeBulkAssign() {
    const modal = document.getElementById('bulkAssignModal');
    if (modal) modal.style.display = 'none';
}

/**
 * Submit the assign dialog: add project/context to the selection.
 */
export async function submitBulkAssign() {
    const project = document.getElementById('bulk-assign-project')?.value.trim() || '';
    const context = document.getElementById('bulk-assign-context')?.value.trim() || '';
    if (!project && !context) return;
    try {
        const payload = { line_indexes: lineIndexes(), markers: markers() };
        if (project) payload.project = project;
        if (context) payload.context = context;
        const result = await runBatch('/api/assign-batch', payload);
        closeBulkAssign();
        const projectInput = document.getElementById('bulk-assign-project');
        const contextInput = document.getElementById('bulk-assign-context');
        if (projectInput) projectInput.value = '';
        if (contextInput) contextInput.value = '';
        await finishAction(translations.bulkAssign, result.updated);
    } catch (err) {
        console.error('Bulk assign failed', err);
    }
}
