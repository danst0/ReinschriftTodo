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
            if (resp.ok && onReloadCallback) {
                onReloadCallback();
            }
        } catch (err) {
            console.error('Undo failed', err);
        }
    });

    document.body.appendChild(toast);

    // Trigger animation
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
