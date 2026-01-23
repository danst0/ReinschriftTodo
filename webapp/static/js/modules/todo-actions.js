/**
 * Todo action handlers - toggle, postpone, delete.
 */

import { fetchWithCsrf } from './api.js';

let onReloadCallback = null;

/**
 * Set the reload callback for after actions.
 * @param {function} fn - Callback function
 */
export function setReloadCallback(fn) {
    onReloadCallback = fn;
}

/**
 * Toggle a todo's completion status.
 * @param {Event} event - Click event
 * @param {number} lineIndex - Line index of the todo
 */
export function toggleTodo(event, lineIndex) {
    event.stopPropagation();
    fetchWithCsrf('/toggle/' + lineIndex, { method: 'POST' })
        .then(() => {
            if (onReloadCallback) {
                onReloadCallback();
            }
        });
}

/**
 * Postpone a todo to a new date.
 * @param {Event} event - Click event
 * @param {number} lineIndex - Line index of the todo
 * @param {string} target - Target date ('today', 'tomorrow', 'weekend', 'sometime')
 */
export function postponeTodo(event, lineIndex, target) {
    event.stopPropagation();
    fetchWithCsrf('/postpone/' + lineIndex + '/' + target, { method: 'POST' })
        .then(() => {
            if (onReloadCallback) {
                onReloadCallback();
            }
        });
}

/**
 * Postpone all todos in a group.
 * @param {Event} event - Click event
 * @param {string} target - Target date
 * @param {string} groupKey - Group key (project or context)
 * @param {string} groupMode - Sort mode ('topic' or 'location')
 */
export async function postponeGroup(event, target, groupKey, groupMode) {
    if (event) {
        event.preventDefault();
        event.stopPropagation();
    }

    if (!groupMode) {
        return;
    }

    const items = Array.from(document.querySelectorAll('.todo-item[data-group-mode]'));
    const matches = items.filter(item => {
        const mode = item.dataset.groupMode || '';
        const key = item.dataset.groupKey || '';
        return mode === groupMode && key === (groupKey || '');
    });

    if (matches.length === 0) {
        return;
    }

    const btn = event && event.currentTarget ? event.currentTarget : null;
    if (btn) {
        btn.style.opacity = '0.6';
        btn.style.pointerEvents = 'none';
    }

    const requests = matches
        .map(item => item.dataset.lineIndex)
        .filter(Boolean)
        .map(idx => fetchWithCsrf('/postpone/' + idx + '/' + target, { method: 'POST' }));

    try {
        await Promise.all(requests);
    } catch (err) {
        console.error('Postpone group failed', err);
    } finally {
        if (btn) {
            btn.style.opacity = '';
            btn.style.pointerEvents = '';
        }
        if (onReloadCallback) {
            onReloadCallback();
        }
    }
}
