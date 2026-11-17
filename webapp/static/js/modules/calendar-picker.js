/**
 * Visual calendar + time picker for datetime-local inputs.
 * Wraps a hidden datetime-local input with a styled trigger button and popup calendar.
 */

const SOMETIME_PREFIX = '9999-12-31';

function pad2(n) {
    return String(n).padStart(2, '0');
}

function parseValue(raw) {
    if (!raw) return null;
    const parts = raw.split('T');
    const dateParts = parts[0].split('-').map(Number);
    const timeParts = (parts[1] || '00:00').split(':').map(Number);
    if (isNaN(dateParts[0]) || isNaN(dateParts[1]) || isNaN(dateParts[2])) return null;
    return {
        year: dateParts[0],
        month: dateParts[1] - 1,
        day: dateParts[2],
        hour: timeParts[0] || 0,
        minute: timeParts[1] || 0
    };
}

function formatValue(y, m, d, h, min) {
    return `${y}-${pad2(m + 1)}-${pad2(d)}T${pad2(h)}:${pad2(min)}`;
}

const CAL_ICON = '<svg class="due-display-icon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="18" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/></svg>';

export function initDuePicker(inputEl, config = {}) {
    const locale = config.locale || 'de';
    const t = config.translations || {};

    const monthNames = Array.from({ length: 12 }, (_, i) =>
        new Intl.DateTimeFormat(locale, { month: 'long' }).format(new Date(2024, i, 1))
    );

    const weekdays = t.weekdays_short || ['Mo', 'Di', 'Mi', 'Do', 'Fr', 'Sa', 'So'];

    let selected = parseValue(inputEl.value);
    const now = new Date();
    let viewYear = selected && selected.year < 9000 ? selected.year : now.getFullYear();
    let viewMonth = selected && selected.year < 9000 ? selected.month : now.getMonth();

    const parent = inputEl.parentNode;
    const wrap = document.createElement('div');
    wrap.className = 'due-picker';

    const trigger = document.createElement('button');
    trigger.type = 'button';
    trigger.className = 'due-display';
    trigger.setAttribute('aria-haspopup', 'dialog');
    trigger.setAttribute('aria-expanded', 'false');
    trigger.innerHTML = CAL_ICON + '<span class="due-display-text"></span>';

    const popup = document.createElement('div');
    popup.className = 'cal-popup';
    popup.setAttribute('role', 'dialog');
    popup.hidden = true;
    popup.innerHTML = [
        '<div class="cal-header">',
        '  <button type="button" class="cal-nav" data-nav="-1" aria-label="&lsaquo;">&lsaquo;</button>',
        '  <div class="cal-title"></div>',
        '  <button type="button" class="cal-nav" data-nav="1" aria-label="&rsaquo;">&rsaquo;</button>',
        '</div>',
        '<div class="cal-weekdays">' + weekdays.map(d => `<span>${d}</span>`).join('') + '</div>',
        '<div class="cal-grid"></div>',
        '<div class="cal-time-row">',
        `  <label class="cal-time-label">${t.time || 'Time'}</label>`,
        '  <input type="time" class="cal-time-input" step="300">',
        '</div>',
        '<div class="cal-footer">',
        `  <button type="button" class="chip is-quiet cal-clear">${t.no_date || 'No date'}</button>`,
        `  <button type="button" class="btn-primary cal-done">${t.picker_done || 'Done'}</button>`,
        '</div>'
    ].join('');

    parent.insertBefore(wrap, inputEl);
    wrap.appendChild(trigger);
    wrap.appendChild(inputEl);
    wrap.appendChild(popup);
    inputEl.classList.add('due-hidden');

    const displayText = trigger.querySelector('.due-display-text');
    const calTitle = popup.querySelector('.cal-title');
    const calGrid = popup.querySelector('.cal-grid');
    const timeInput = popup.querySelector('.cal-time-input');

    function updateDisplay() {
        const raw = inputEl.value;
        if (!raw) {
            displayText.textContent = t.select_date || 'Select date\u2026';
            trigger.classList.add('is-empty');
        } else if (raw.startsWith(SOMETIME_PREFIX)) {
            displayText.textContent = t.sometime || 'Sometime';
            trigger.classList.remove('is-empty');
        } else {
            const d = parseValue(raw);
            if (!d) {
                displayText.textContent = raw;
                trigger.classList.remove('is-empty');
                return;
            }
            const date = new Date(d.year, d.month, d.day);
            const dateStr = new Intl.DateTimeFormat(locale, {
                weekday: 'short', day: 'numeric', month: 'long', year: 'numeric'
            }).format(date);
            displayText.textContent = `${dateStr} \u00b7 ${pad2(d.hour)}:${pad2(d.minute)}`;
            trigger.classList.remove('is-empty');
        }
    }

    function renderCalendar() {
        calTitle.textContent = `${monthNames[viewMonth]} ${viewYear}`;
        const firstDay = new Date(viewYear, viewMonth, 1);
        const daysInMonth = new Date(viewYear, viewMonth + 1, 0).getDate();
        const startDow = (firstDay.getDay() + 6) % 7;
        const today = new Date();
        const sel = parseValue(inputEl.value);
        const selValid = sel && sel.year < 9000;

        let html = '';
        for (let i = 0; i < startDow; i++) {
            html += '<span class="cal-cell is-empty"></span>';
        }
        for (let d = 1; d <= daysInMonth; d++) {
            const isToday = today.getFullYear() === viewYear
                && today.getMonth() === viewMonth
                && today.getDate() === d;
            const isSelected = selValid
                && sel.year === viewYear
                && sel.month === viewMonth
                && sel.day === d;
            const cls = ['cal-cell'];
            if (isToday) cls.push('is-today');
            if (isSelected) cls.push('is-selected');
            html += `<button type="button" class="${cls.join(' ')}" data-day="${d}">${d}</button>`;
        }
        calGrid.innerHTML = html;
    }

    function syncTimeInput() {
        const sel = parseValue(inputEl.value);
        if (sel && sel.year < 9000) {
            timeInput.value = `${pad2(sel.hour)}:${pad2(sel.minute)}`;
        } else {
            timeInput.value = '12:00';
        }
    }

    function openPopup() {
        const sel = parseValue(inputEl.value);
        if (sel && sel.year < 9000) {
            viewYear = sel.year;
            viewMonth = sel.month;
        } else {
            const n = new Date();
            viewYear = n.getFullYear();
            viewMonth = n.getMonth();
        }
        renderCalendar();
        syncTimeInput();
        popup.hidden = false;
        trigger.setAttribute('aria-expanded', 'true');
        trigger.classList.add('is-open');
        setTimeout(() => document.addEventListener('click', handleOutsideClick), 0);
    }

    function closePopup() {
        popup.hidden = true;
        trigger.setAttribute('aria-expanded', 'false');
        trigger.classList.remove('is-open');
        document.removeEventListener('click', handleOutsideClick);
    }

    function handleOutsideClick(e) {
        if (!wrap.contains(e.target)) closePopup();
    }

    function selectDay(day) {
        const time = timeInput.value || '12:00';
        const [h, m] = time.split(':').map(Number);
        inputEl.value = formatValue(viewYear, viewMonth, day, h || 0, m || 0);
        renderCalendar();
        updateDisplay();
    }

    function applyTime() {
        const sel = parseValue(inputEl.value);
        if (!sel || sel.year >= 9000) return;
        const time = timeInput.value || '12:00';
        const [h, m] = time.split(':').map(Number);
        inputEl.value = formatValue(sel.year, sel.month, sel.day, h || 0, m || 0);
        updateDisplay();
    }

    trigger.addEventListener('click', (e) => {
        e.stopPropagation();
        if (popup.hidden) openPopup();
        else closePopup();
    });

    popup.addEventListener('click', (e) => {
        e.stopPropagation();
        const nav = e.target.closest('[data-nav]');
        if (nav) {
            viewMonth += Number(nav.dataset.nav);
            if (viewMonth < 0) { viewMonth = 11; viewYear--; }
            else if (viewMonth > 11) { viewMonth = 0; viewYear++; }
            renderCalendar();
            return;
        }
        const dayBtn = e.target.closest('[data-day]');
        if (dayBtn) {
            selectDay(Number(dayBtn.dataset.day));
            return;
        }
        if (e.target.closest('.cal-clear')) {
            inputEl.value = '';
            updateDisplay();
            renderCalendar();
            closePopup();
            return;
        }
        if (e.target.closest('.cal-done')) {
            closePopup();
        }
    });

    timeInput.addEventListener('change', applyTime);

    popup.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
            closePopup();
            trigger.focus();
        }
    });

    const valueDesc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
    Object.defineProperty(inputEl, 'value', {
        get() { return valueDesc.get.call(this); },
        set(v) {
            valueDesc.set.call(this, v);
            updateDisplay();
            if (!popup.hidden) {
                const s = parseValue(v);
                if (s && s.year < 9000) {
                    viewYear = s.year;
                    viewMonth = s.month;
                }
                renderCalendar();
                syncTimeInput();
            }
        }
    });

    updateDisplay();

    return { open: openPopup, close: closePopup, update: updateDisplay };
}