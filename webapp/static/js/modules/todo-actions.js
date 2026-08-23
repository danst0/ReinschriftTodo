/**
 * Todo action handlers - toggle, postpone, delete.
 */

import { fetchWithCsrf } from './api.js';
import { showToast } from './undo-toast.js';

let onReloadCallback = null;
let messages = {};

/**
 * Set the reload callback for after actions.
 * @param {function} fn - Callback function
 */
export function setReloadCallback(fn) {
    onReloadCallback = fn;
}

/**
 * Provide translated failure messages.
 * @param {object} [options] - {translations}
 */
export function configureTodoActions(options = {}) {
    messages = options.translations || {};
}

/**
 * Report a write that did not land.
 *
 * Reloading after a failed request used to make it look like the click was
 * undone: the request had been rejected, nothing was written, and the partial
 * reload simply showed the unchanged file again. Say what happened instead.
 *
 * @param {Response|null} response - The failed response, or null on a network error
 */
function reportFailure(response) {
    const status = response ? response.status : 0;

    let message;
    if (status === 409) {
        message = messages.actionConflict || 'Die Aufgabe wurde inzwischen woanders geändert.';
    } else if (status === 401) {
        message = messages.actionUnauthorized || 'Sitzung abgelaufen. Bitte neu anmelden.';
    } else {
        message = messages.actionFailed || 'Konnte nicht speichern.';
    }

    showToast(message, 5000, true);
}

/**
 * POST a single-todo action and report it if it fails.
 *
 * The marker travels with the line index because the index describes the page
 * as it was rendered. If the file changed since, the index points at whatever
 * moved into that slot; the marker still points at the todo that was clicked.
 *
 * @param {string} url - Action URL
 * @param {string} marker - Marker ID of the todo, may be empty
 * @returns {Promise<void>}
 */
async function submitAction(url, marker) {
    const body = new URLSearchParams();
    if (marker) {
        body.set('marker', marker);
    }

    try {
        const res = await fetchWithCsrf(url, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
                // Makes an expired login answer 401 instead of quietly
                // redirecting to the login page with a 200.
                'X-Requested-With': 'XMLHttpRequest'
            },
            body: body.toString()
        });
        if (!res.ok) {
            reportFailure(res);
        }
    } catch (err) {
        console.error('Todo action failed', err);
        reportFailure(null);
    } finally {
        // Reload either way: on success it shows the new state, on failure the
        // state that actually stands.
        if (onReloadCallback) {
            onReloadCallback();
        }
    }
}

/**
 * Toggle a todo's completion status.
 * @param {Event} event - Click event
 * @param {number} lineIndex - Line index of the todo
 * @param {string} [marker] - Marker ID of the todo
 * @returns {Promise<void>}
 */
export async function toggleTodo(event, lineIndex, marker = '') {
    if (event && event.stopPropagation) {
        event.stopPropagation();
    }
    await submitAction('/toggle/' + lineIndex, marker);
}

/**
 * Postpone a todo to a new date.
 * @param {Event} event - Click event
 * @param {number} lineIndex - Line index of the todo
 * @param {string} target - Target date ('today', 'tomorrow', 'weekend', 'sometime')
 * @param {string} [marker] - Marker ID of the todo
 * @returns {Promise<void>}
 */
export async function postponeTodo(event, lineIndex, target, marker = '') {
    if (event && event.stopPropagation) {
        event.stopPropagation();
    }
    await submitAction('/postpone/' + lineIndex + '/' + target, marker);
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

    const selected = matches.filter(item => item.dataset.lineIndex);
    const lineIndexes = selected.map(item => parseInt(item.dataset.lineIndex, 10));
    const markers = selected.map(item => item.dataset.marker || '');

    try {
        const res = await fetchWithCsrf('/api/postpone-batch', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                line_indexes: lineIndexes,
                markers: markers,
                target: target
            })
        });
        if (!res.ok) {
            reportFailure(res);
        }
    } catch (err) {
        console.error('Postpone group failed', err);
        reportFailure(null);
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
