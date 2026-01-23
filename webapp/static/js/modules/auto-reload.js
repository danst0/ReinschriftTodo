/**
 * Auto-reload functionality for background refresh.
 */

let isRefreshing = false;
let autoReloadInterval = null;
let onReloadComplete = null;

/**
 * Perform an auto-reload of the todo list.
 * @returns {Promise<void>}
 */
export async function autoReload() {
    if (isRefreshing) return;
    isRefreshing = true;

    try {
        // Construct URL with current params + partial=1
        const url = new URL(window.location.href);
        url.searchParams.set('partial', '1');

        const response = await fetch(url);
        const html = await response.text();

        const todoList = document.querySelector('.todo-list');
        if (todoList) {
            todoList.innerHTML = html;
        }

        if (onReloadComplete) {
            onReloadComplete();
        }
    } catch (err) {
        console.error('Auto-reload failed', err);
    } finally {
        isRefreshing = false;
    }
}

/**
 * Start auto-reload interval.
 * @param {number} intervalMs - Interval in milliseconds (default: 30000)
 * @param {function} callback - Optional callback after each reload
 */
export function startAutoReload(intervalMs = 30000, callback = null) {
    onReloadComplete = callback;

    if (autoReloadInterval) {
        clearInterval(autoReloadInterval);
    }

    autoReloadInterval = setInterval(() => {
        const modalOpen = document.getElementById('editModal')?.style.display === 'block';
        const settingsOpen = document.getElementById('settingsModal')?.style.display === 'block';

        if (!document.hidden && !modalOpen && !settingsOpen) {
            autoReload();
        }
    }, intervalMs);
}

/**
 * Stop auto-reload interval.
 */
export function stopAutoReload() {
    if (autoReloadInterval) {
        clearInterval(autoReloadInterval);
        autoReloadInterval = null;
    }
}

/**
 * Manual reload (full page).
 */
export function manualReload() {
    window.location.reload();
}

/**
 * Set reload complete callback.
 * @param {function} callback
 */
export function setReloadCallback(callback) {
    onReloadComplete = callback;
}
