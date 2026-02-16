/**
 * Browser notification support for due date reminders.
 */

let checkInterval = null;
const notifiedItems = new Set();

/**
 * Start checking for due items at regular intervals.
 * @param {number} [intervalMs=300000] - Check interval in milliseconds (default 5 min)
 */
export function startNotificationCheck(intervalMs = 300000) {
    if (!('Notification' in window)) {
        return;
    }

    // Request permission on first call
    if (Notification.permission === 'default') {
        Notification.requestPermission();
    }

    checkDueItems();
    checkInterval = setInterval(checkDueItems, intervalMs);
}

/**
 * Stop notification checks.
 */
export function stopNotificationCheck() {
    if (checkInterval) {
        clearInterval(checkInterval);
        checkInterval = null;
    }
}

/**
 * Check all visible todo items for due/overdue status.
 */
function checkDueItems() {
    if (Notification.permission !== 'granted') {
        return;
    }

    const now = new Date();
    const items = document.querySelectorAll('.todo-item[data-due]');

    items.forEach(item => {
        const dueStr = item.dataset.due;
        if (!dueStr) return;

        const dueDate = new Date(dueStr);
        if (isNaN(dueDate.getTime())) return;

        // Skip "sometime" sentinel (year 9999)
        if (dueDate.getFullYear() >= 9999) return;

        // Skip done items
        if (item.dataset.done === 'true') return;

        const key = item.dataset.marker || item.dataset.lineIndex || dueStr;
        if (notifiedItems.has(key)) return;

        // Notify if due within 30 minutes or overdue
        const thirtyMin = 30 * 60 * 1000;
        if (dueDate.getTime() - now.getTime() <= thirtyMin) {
            notifiedItems.add(key);
            const title = item.dataset.title || item.querySelector('.todo-title')?.textContent || 'Task due';
            new Notification('Task due', {
                body: title,
                tag: key,
                icon: '/static/favicon.ico'
            });
        }
    });
}
