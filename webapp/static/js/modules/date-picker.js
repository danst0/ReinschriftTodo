/**
 * Date picker utilities - shared between modal and standalone edit page.
 */

/**
 * Extract time portion from datetime input value.
 * @param {HTMLInputElement|null} inputEl - Datetime input element
 * @returns {string} Time string (HH:MM) or '00:00'
 */
export function timePart(inputEl) {
    if (!inputEl) return '00:00';
    const raw = inputEl.value || '';
    if (raw.includes('T')) {
        const maybeTime = raw.split('T')[1].slice(0, 5);
        return maybeTime || '00:00';
    }
    return '00:00';
}

/**
 * Set a date input to today or a future date.
 * @param {HTMLInputElement} dateInput - The datetime-local input
 * @param {number} offset - Days offset (0 = today, 1 = tomorrow, etc.)
 */
export function setDate(dateInput, offset) {
    if (!dateInput) return;

    const now = new Date();
    let targetDate = new Date();
    let targetTime;

    if (offset === 0) {
        // Smart "Today" logic: at least 4 hours from now
        const currentHour = now.getHours();
        if (currentHour < 8) {
            targetTime = '12:00';
        } else if (currentHour < 14) {
            targetTime = '18:00';
        } else {
            targetTime = '18:00';
        }
    } else {
        targetDate.setDate(now.getDate() + offset);
        targetTime = '12:00';
    }

    const yyyy = targetDate.getFullYear();
    const mm = String(targetDate.getMonth() + 1).padStart(2, '0');
    const dd = String(targetDate.getDate()).padStart(2, '0');
    dateInput.value = `${yyyy}-${mm}-${dd}T${targetTime}`;
}

/**
 * Set a date input to the next weekend (Saturday).
 * @param {HTMLInputElement} dateInput - The datetime-local input
 */
export function setNextWeekend(dateInput) {
    if (!dateInput) return;

    const now = new Date();
    const dayOfWeek = now.getDay(); // 0 = Sunday, 6 = Saturday
    let daysUntilSaturday;

    if (dayOfWeek === 6) {
        // Today is Saturday, go to next Saturday
        daysUntilSaturday = 7;
    } else if (dayOfWeek === 0) {
        // Today is Sunday, go to next Saturday (6 days)
        daysUntilSaturday = 6;
    } else {
        // Monday-Friday: days until this Saturday
        daysUntilSaturday = 6 - dayOfWeek;
    }

    const targetDate = new Date(now);
    targetDate.setDate(now.getDate() + daysUntilSaturday);

    const yyyy = targetDate.getFullYear();
    const mm = String(targetDate.getMonth() + 1).padStart(2, '0');
    const dd = String(targetDate.getDate()).padStart(2, '0');
    dateInput.value = `${yyyy}-${mm}-${dd}T12:00`;
}

/**
 * Set a date input to "sometime" (far future sentinel value).
 * @param {HTMLInputElement} dateInput - The datetime-local input
 */
export function setSometime(dateInput) {
    if (!dateInput) return;
    dateInput.value = '9999-12-31T00:00';
}

/**
 * Navigate focus between form elements.
 * @param {HTMLFormElement} form - Parent form
 * @param {HTMLElement} current - Currently focused element
 * @param {boolean} backwards - Tab backwards
 */
export function focusRelative(form, current, backwards) {
    const focusables = Array.from(form.querySelectorAll('input, select, textarea, button'));
    const idx = focusables.indexOf(current);
    if (idx === -1) return;
    const nextIdx = backwards ? idx - 1 : idx + 1;
    if (nextIdx >= 0 && nextIdx < focusables.length) {
        focusables[nextIdx].focus();
    }
}
