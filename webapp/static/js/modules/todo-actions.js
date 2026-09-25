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
 * Writes that are already on screen but not yet sent, in click order.
 *
 * A click used to wait for the server and then for a reload before the list
 * changed at all. Now the row changes right away and the request goes into
 * this queue. Requests leave one at a time: two at once would race on the same
 * file, and the second would come back as a conflict.
 */
const queue = [];
let sending = false;
let outstanding = 0;

/**
 * Whether changes are queued or still on their way to the server.
 * A reload in that window would show the file without them.
 * @returns {boolean}
 */
export function hasPendingWrites() {
    return queue.length > 0 || outstanding > 0;
}

/**
 * Send one queued request.
 *
 * `keepalive` lets the browser finish the request even when the page is
 * closed or left while it is under way.
 *
 * @param {{url: string, init: object}} job
 * @returns {Promise<void>}
 */
async function send(job) {
    outstanding += 1;
    try {
        const res = await fetchWithCsrf(job.url, { ...job.init, keepalive: true });
        if (!res.ok) {
            reportFailure(res);
        }
    } catch (err) {
        console.error('Todo action failed', err);
        reportFailure(null);
    } finally {
        outstanding -= 1;
    }
}

async function pump() {
    if (sending) return;
    const job = queue.shift();
    if (!job) {
        // Everything is stored: show the state that actually stands — on
        // failure the unchanged one, on success e.g. a recurring task's next
        // occurrence.
        if (outstanding === 0 && onReloadCallback) {
            onReloadCallback();
        }
        return;
    }
    sending = true;
    try {
        await send(job);
    } finally {
        sending = false;
        pump();
    }
}

/**
 * Queue a write and start sending if idle.
 * @param {string} url
 * @param {object} init - fetch options
 */
function enqueue(url, init) {
    queue.push({ url, init });
    pump();
}

/**
 * Send everything still queued, right now and in parallel.
 *
 * On a closed tab or a phone switching apps the queue would never get another
 * turn. Requests already under way finish on their own thanks to keepalive.
 */
function flush() {
    const jobs = queue.splice(0);
    if (jobs.length === 0) return;
    Promise.all(jobs.map(send)).then(() => {
        if (onReloadCallback && !document.hidden) onReloadCallback();
    });
}

window.addEventListener('pagehide', flush);
document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') flush();
});
window.addEventListener('beforeunload', (event) => {
    // Only unsent changes are at risk; those already under way complete.
    if (queue.length > 0) {
        event.preventDefault();
        event.returnValue = '';
    }
});

/**
 * Find the row a click was about, marker first like the server does.
 * @param {number|string} lineIndex
 * @param {string} marker
 * @returns {HTMLElement|null}
 */
function findRow(lineIndex, marker) {
    if (marker) {
        const row = document.querySelector(`.todo-item[data-marker="${CSS.escape(marker)}"]`);
        if (row) return row;
    }
    return document.querySelector(`.todo-item[data-line-index="${lineIndex}"]`);
}

/**
 * Queue a single-todo action.
 *
 * The marker travels with the line index because the index describes the page
 * as it was rendered. If the file changed since, the index points at whatever
 * moved into that slot; the marker still points at the todo that was clicked.
 *
 * @param {string} url - Action URL
 * @param {string} marker - Marker ID of the todo, may be empty
 */
function submitAction(url, marker) {
    const body = new URLSearchParams();
    if (marker) {
        body.set('marker', marker);
    }

    enqueue(url, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/x-www-form-urlencoded',
            // Makes an expired login answer 401 instead of quietly
            // redirecting to the login page with a 200, and gets a bare 204
            // instead of a redirect to the full index page.
            'X-Requested-With': 'XMLHttpRequest'
        },
        body: body.toString()
    });
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
    const row = findRow(lineIndex, marker);
    if (row) {
        const done = !row.classList.contains('done');
        row.classList.toggle('done', done);
        const box = row.querySelector('.checkbox');
        if (box) {
            box.classList.toggle('unchecked', !done);
            box.textContent = done ? '☑' : '☐';
        }
    }
    submitAction('/toggle/' + lineIndex, marker);
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
    // The row's new place depends on the sort order; fade it until the
    // reload after the write puts it there.
    findRow(lineIndex, marker)?.classList.add('is-pending');
    submitAction('/postpone/' + lineIndex + '/' + target, marker);
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

    const selected = matches.filter(item => item.dataset.lineIndex);
    const lineIndexes = selected.map(item => parseInt(item.dataset.lineIndex, 10));
    const markers = selected.map(item => item.dataset.marker || '');
    selected.forEach(item => item.classList.add('is-pending'));

    enqueue('/api/postpone-batch', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            line_indexes: lineIndexes,
            markers: markers,
            target: target
        })
    });
}
