/**
 * "My Day" (Mein Tag) planning actions.
 */

import { postJson } from './api.js';

let onReload = null;

/**
 * Set the reload callback invoked after a successful toggle.
 * @param {function} callback
 */
export function setMydayReloadCallback(callback) {
    onReload = callback;
}

/**
 * Add a todo to today's plan.
 * @param {Event} event - Click event
 * @param {string} marker - Todo marker ID
 */
export function addToMyDay(event, marker) {
    event.preventDefault();
    toggleMyDay(marker, true, event.currentTarget);
}

/**
 * Remove a todo from today's plan.
 * @param {Event} event - Click event
 * @param {string} marker - Todo marker ID
 */
export function removeFromMyDay(event, marker) {
    event.preventDefault();
    toggleMyDay(marker, false, event.currentTarget);
}

async function toggleMyDay(marker, on, sourceEl) {
    if (!marker) return;

    // Optimistic feedback: fade the affected row until the partial reload lands.
    const row = sourceEl?.closest('.myday-picker-item, .todo-item');
    if (row) row.classList.add('myday-pending');

    try {
        const res = await postJson('/api/myday/toggle', { marker, on });
        if (!res.ok) {
            throw new Error(`HTTP ${res.status}`);
        }
        if (onReload) {
            await onReload();
        }
    } catch (err) {
        console.error('My Day toggle failed', err);
        if (row) row.classList.remove('myday-pending');
    }
}
