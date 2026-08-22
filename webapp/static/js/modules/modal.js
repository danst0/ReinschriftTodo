/**
 * Edit modal management.
 */

import { getCsrfToken, fetchWithCsrf } from './api.js';
import { setDate, setNextWeekend, setSometime, focusRelative } from './date-picker.js';

let modal = null;
let onReloadCallback = null;

/**
 * Initialize modal with DOM element and reload callback.
 * @param {HTMLElement} modalElement - Modal DOM element
 * @param {function} reloadFn - Function to call after save/delete
 */
export function initModal(modalElement, reloadFn) {
    modal = modalElement;
    onReloadCallback = reloadFn;

    // Set up form submission
    const editForm = document.getElementById('editForm');
    if (editForm) {
        editForm.onsubmit = handleFormSubmit;
        editForm.addEventListener('keydown', handleFormKeydown);
    }

    // Set up note field Tab handling
    const editNote = document.getElementById('edit-note');
    if (editNote) {
        editNote.addEventListener('keydown', handleNoteKeydown);
    }

    // Close modal on outside click
    window.addEventListener('click', (event) => {
        if (event.target === modal) {
            closeModal();
        }
    });
}

/**
 * Open edit modal for a todo.
 * @param {number} lineIndex - Line index of the todo
 * @param {object} config - Configuration with API URL and translations
 */
export function openEditModal(lineIndex, config = {}) {
    fetchWithCsrf('/api/todo/' + lineIndex)
        .then(response => response.json())
        .then(data => {
            if (data.error) {
                alert((config.errorLoading || 'Error loading: ') + data.error);
                return;
            }

            document.getElementById('edit-title').value = data.title;

            // Format projects with + prefix — the '+' delimits the names on save,
            // so spaces inside a name survive the round-trip (issue #9).
            const projects = data.projects || [];
            document.getElementById('edit-projects').value = projects.map(p => `+${p}`).join(' ');

            // Same for contexts with the @ prefix.
            const contexts = data.contexts || [];
            document.getElementById('edit-contexts').value = contexts.map(c => `@${c}`).join(' ');

            document.getElementById('edit-due').value = data.due || '';
            document.getElementById('edit-recurrence').value = data.recurrence || '';
            document.getElementById('edit-reference').value = data.reference || '';
            document.getElementById('edit-note').value = data.note || '';
            document.getElementById('edit-done').checked = data.done;
            document.getElementById('btn-close-comment').style.display = data.done ? 'none' : 'inline-block';
            document.getElementById('btn-save-main').style.display = 'inline-block';
            document.getElementById('comment-section').style.display = 'none';

            // Save by marker: the index this modal was opened with can go
            // stale if the file changes while the list is open.
            const markerField = document.getElementById('edit-marker');
            if (markerField) {
                markerField.value = data.marker || '';
            }
            document.getElementById('editForm').action = '/edit/' + lineIndex;

            modal.style.display = 'block';
        })
        .catch(error => {
            console.error('Error:', error);
            alert(config.failedDetails || 'Failed to load todo details');
        });
}

/**
 * Close the edit modal.
 */
export function closeModal() {
    if (modal) {
        modal.style.display = 'none';
    }
    document.getElementById('comment-section').style.display = 'none';
    document.getElementById('btn-close-comment').style.display = 'inline-block';
    document.getElementById('btn-save-main').style.display = 'inline-block';
    document.getElementById('edit-note').value = '';
    document.getElementById('edit-comment').value = '';
}

/**
 * Show the comment field for closing with comment.
 */
export function showCommentField() {
    document.getElementById('comment-section').style.display = 'block';
    document.getElementById('edit-done').checked = true;
    document.getElementById('btn-close-comment').style.display = 'none';
    document.getElementById('btn-save-main').style.display = 'none';
    const commentInput = document.getElementById('edit-comment');
    commentInput.focus();
    setTimeout(() => {
        commentInput.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }, 300);
}

/**
 * Submit the edit form with comment.
 */
export function submitWithComment() {
    const form = document.getElementById('editForm');
    form.dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }));
}

/**
 * Delete todo from modal.
 * @param {boolean} skipConfirm - Skip confirmation dialog
 * @param {string} confirmMessage - Confirmation message to show
 */
export function deleteTodoFromModal(skipConfirm, confirmMessage) {
    const action = document.getElementById('editForm').action;
    const lineIndex = action.split('/').pop();
    const shouldDelete = skipConfirm || confirm(confirmMessage || 'Delete this item?');

    if (!shouldDelete) {
        return;
    }

    fetchWithCsrf('/delete/' + lineIndex, { method: 'POST' })
        .then(() => {
            closeModal();
            if (onReloadCallback) {
                onReloadCallback();
            }
        });
}

// Modal date picker helpers
export function setModalDate(offset) {
    setDate(document.getElementById('edit-due'), offset);
}

export function setModalNextWeekend() {
    setNextWeekend(document.getElementById('edit-due'));
}

export function setModalSometime() {
    setSometime(document.getElementById('edit-due'));
}

// Private handlers
function handleFormSubmit(e) {
    e.preventDefault();
    const formData = new FormData(this);
    fetch(this.action, {
        method: 'POST',
        body: formData
    }).then(() => {
        closeModal();
        if (onReloadCallback) {
            onReloadCallback();
        }
    });
}

function handleFormKeydown(e) {
    if (e.key === 'Enter' && e.target.tagName === 'INPUT') {
        e.preventDefault();
        this.requestSubmit();
    }
}

function handleNoteKeydown(e) {
    if (e.key === 'Tab') {
        e.preventDefault();
        focusRelative(document.getElementById('editForm'), e.target, e.shiftKey);
    } else if ((e.key === 'Enter' || e.key === 'NumpadEnter') && e.ctrlKey) {
        e.preventDefault();
        document.getElementById('editForm').dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }));
    }
}
