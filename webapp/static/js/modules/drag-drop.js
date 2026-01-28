/**
 * Drag-and-drop between section groups (projects/contexts).
 */

import { fetchWithCsrf } from './api.js';

let reloadCallback = null;
let abortController = null;

/**
 * Set the callback to reload the todo list after a drop.
 * @param {function} callback
 */
export function setDragReloadCallback(callback) {
    reloadCallback = callback;
}

/**
 * Initialize drag-and-drop on all drag handles.
 * Call after each reload to rebind events.
 */
export function initDragDrop() {
    destroyDragDrop();
    abortController = new AbortController();
    const signal = abortController.signal;

    const handles = document.querySelectorAll('.drag-handle');
    if (handles.length === 0) return;

    handles.forEach(handle => {
        // Mouse events
        handle.addEventListener('mousedown', onMouseDown, { signal });

        // Touch events (long-press to start)
        handle.addEventListener('touchstart', onTouchStart, { passive: true, signal });
    });
}

/**
 * Remove all drag-drop event listeners.
 */
export function destroyDragDrop() {
    if (abortController) {
        abortController.abort();
        abortController = null;
    }
    cleanup();
}

// --- Internal state ---
let dragState = null;

function getTodoItem(handle) {
    return handle.closest('.todo-item');
}

function getContainer(el) {
    return el ? el.closest('.section-items') : null;
}

// --- Mouse flow ---

function onMouseDown(e) {
    e.preventDefault();
    e.stopPropagation();
    const item = getTodoItem(e.target);
    if (!item) return;
    startDrag(item, e.clientX, e.clientY);
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
}

function onMouseMove(e) {
    if (!dragState) return;
    moveDrag(e.clientX, e.clientY);
}

function onMouseUp(e) {
    document.removeEventListener('mousemove', onMouseMove);
    document.removeEventListener('mouseup', onMouseUp);
    if (!dragState) return;
    endDrag(e.clientX, e.clientY);
}

// --- Touch flow (long-press) ---

let touchTimer = null;
let touchStartX = 0;
let touchStartY = 0;

function onTouchStart(e) {
    if (e.touches.length > 1) return;
    const touch = e.touches[0];
    touchStartX = touch.clientX;
    touchStartY = touch.clientY;
    const item = getTodoItem(e.target);
    if (!item) return;

    touchTimer = setTimeout(() => {
        startDrag(item, touch.clientX, touch.clientY);
        document.addEventListener('touchmove', onTouchMove, { passive: false });
        document.addEventListener('touchend', onTouchEnd);
        document.addEventListener('touchcancel', onTouchCancel);
    }, 200);

    // Cancel long-press if finger moves too much
    const cancelLongPress = (ev) => {
        const dx = ev.touches[0].clientX - touchStartX;
        const dy = ev.touches[0].clientY - touchStartY;
        if (Math.abs(dx) > 10 || Math.abs(dy) > 10) {
            clearTimeout(touchTimer);
            touchTimer = null;
            document.removeEventListener('touchmove', cancelLongPress);
        }
    };
    document.addEventListener('touchmove', cancelLongPress, { passive: true });

    // Also clear on touchend before long-press fires
    const cancelOnEnd = () => {
        clearTimeout(touchTimer);
        touchTimer = null;
        document.removeEventListener('touchend', cancelOnEnd);
        document.removeEventListener('touchmove', cancelLongPress);
    };
    document.addEventListener('touchend', cancelOnEnd, { passive: true });
}

function onTouchMove(e) {
    if (!dragState) return;
    e.preventDefault();
    const touch = e.touches[0];
    moveDrag(touch.clientX, touch.clientY);
}

function onTouchEnd(e) {
    document.removeEventListener('touchmove', onTouchMove);
    document.removeEventListener('touchend', onTouchEnd);
    document.removeEventListener('touchcancel', onTouchCancel);
    if (!dragState) return;
    const touch = e.changedTouches[0];
    endDrag(touch.clientX, touch.clientY);
}

function onTouchCancel() {
    document.removeEventListener('touchmove', onTouchMove);
    document.removeEventListener('touchend', onTouchEnd);
    document.removeEventListener('touchcancel', onTouchCancel);
    cleanup();
}

// --- Core drag logic ---

function startDrag(item, x, y) {
    const rect = item.getBoundingClientRect();
    const clone = item.cloneNode(true);
    clone.classList.add('drag-clone');
    clone.style.width = rect.width + 'px';
    clone.style.left = rect.left + 'px';
    clone.style.top = rect.top + 'px';
    document.body.appendChild(clone);

    item.classList.add('drag-ghost');

    dragState = {
        item,
        clone,
        offsetX: x - rect.left,
        offsetY: y - rect.top,
        sourceContainer: getContainer(item),
        currentOverContainer: null
    };
}

function moveDrag(x, y) {
    if (!dragState) return;
    const { clone, offsetX, offsetY } = dragState;
    clone.style.left = (x - offsetX) + 'px';
    clone.style.top = (y - offsetY) + 'px';

    // Find target container under cursor
    clone.style.pointerEvents = 'none';
    const el = document.elementFromPoint(x, y);
    clone.style.pointerEvents = '';

    const targetContainer = el ? el.closest('.section-items') : null;

    if (dragState.currentOverContainer && dragState.currentOverContainer !== targetContainer) {
        dragState.currentOverContainer.classList.remove('drag-over');
    }
    if (targetContainer && targetContainer !== dragState.sourceContainer) {
        targetContainer.classList.add('drag-over');
    }
    dragState.currentOverContainer = targetContainer;
}

function endDrag(x, y) {
    if (!dragState) return;
    const { item, clone, sourceContainer, currentOverContainer } = dragState;

    // Determine target
    clone.style.pointerEvents = 'none';
    const el = document.elementFromPoint(x, y);
    clone.style.pointerEvents = '';
    const targetContainer = el ? el.closest('.section-items') : currentOverContainer;

    cleanup();

    if (!targetContainer || targetContainer === sourceContainer) return;

    const marker = item.dataset.marker;
    if (!marker) return;

    const groupMode = sourceContainer.dataset.groupMode;
    const fromKey = sourceContainer.dataset.groupKey;
    const toKey = targetContainer.dataset.groupKey;

    if (fromKey === toKey) return;

    handleDrop(marker, groupMode, fromKey, toKey);
}

function cleanup() {
    if (dragState) {
        dragState.item.classList.remove('drag-ghost');
        if (dragState.clone && dragState.clone.parentNode) {
            dragState.clone.parentNode.removeChild(dragState.clone);
        }
        if (dragState.currentOverContainer) {
            dragState.currentOverContainer.classList.remove('drag-over');
        }
        dragState = null;
    }
    document.querySelectorAll('.drag-over').forEach(el => el.classList.remove('drag-over'));
}

async function handleDrop(marker, groupMode, fromKey, toKey) {
    try {
        const res = await fetchWithCsrf('/api/move-to-section', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                marker,
                group_mode: groupMode,
                from_key: fromKey,
                to_key: toKey
            })
        });

        if (!res.ok) {
            console.error('Move failed', await res.text());
            return;
        }

        if (reloadCallback) {
            await reloadCallback();
        }
    } catch (err) {
        console.error('Move failed', err);
    }
}
