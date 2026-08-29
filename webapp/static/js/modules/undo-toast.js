/**
 * Undo toast notification for destructive actions.
 */

import { fetchWithCsrf } from './api.js';

let toastTimeout = null;
let onReloadCallback = null;

/**
 * Set the reload callback for after undo.
 * @param {Function} fn - Callback function
 */
export function setUndoReloadCallback(fn) {
    onReloadCallback = fn;
}

/**
 * Show an undo toast notification.
 * @param {string} message - Message to display
 * @param {number} [duration=8000] - Toast duration in ms
 */
export function showUndoToast(message, duration = 8000) {
    removeToast();

    const toast = document.createElement('div');
    toast.className = 'undo-toast';
    toast.innerHTML = `
        <span class="undo-toast-message">${message}</span>
        <button class="undo-toast-btn" type="button">Undo</button>
    `;

    toast.querySelector('.undo-toast-btn').addEventListener('click', async () => {
        removeToast();
        try {
            const resp = await fetchWithCsrf('/api/undo', { method: 'POST' });
            if (!resp.ok) {
                // A refused undo — the lines have moved on since — must say so.
                // Staying silent looks exactly like an undo that worked.
                const data = await resp.json().catch(() => ({}));
                showToast(data.error || 'Undo failed', 5000, true);
                return;
            }
            if (onReloadCallback) {
                onReloadCallback();
            }
        } catch (err) {
            console.error('Undo failed', err);
            showToast('Undo failed', 5000, true);
        }
    });

    document.body.appendChild(toast);

    // Trigger animation
    requestAnimationFrame(() => toast.classList.add('visible'));

    toastTimeout = setTimeout(() => removeToast(), duration);
}

/**
 * Show a plain message toast, with no action attached.
 *
 * Used to report an action that did not go through. A write that fails must
 * say so — reloading on a silent failure just makes the change look undone.
 *
 * @param {string} message - Message to display
 * @param {number} [duration=5000] - Toast duration in ms
 * @param {boolean} [isError=false] - Style it as a failure
 */
export function showToast(message, duration = 5000, isError = false) {
    removeToast();

    const toast = document.createElement('div');
    toast.className = isError ? 'undo-toast is-error' : 'undo-toast';
    toast.setAttribute('role', 'status');

    const text = document.createElement('span');
    text.className = 'undo-toast-message';
    text.textContent = message;
    toast.appendChild(text);

    document.body.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add('visible'));

    toastTimeout = setTimeout(() => removeToast(), duration);
}

/**
 * Remove the current toast if present.
 */
function removeToast() {
    if (toastTimeout) {
        clearTimeout(toastTimeout);
        toastTimeout = null;
    }
    const existing = document.querySelector('.undo-toast');
    if (existing) {
        existing.remove();
    }
}
