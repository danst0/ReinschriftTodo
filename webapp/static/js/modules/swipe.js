/**
 * Swipe gestures for todo items.
 */

import { toggleTodo, postponeTodo } from './todo-actions.js';

const SWIPE_THRESHOLD = 80;
const SWIPE_MAX = 120;

/**
 * Initialize swipe gestures on all todo items.
 */
export function initSwipeGestures() {
    const todoItems = document.querySelectorAll('.todo-item');

    todoItems.forEach(item => {
        let startX = 0;
        let startY = 0;
        let currentX = 0;
        let isDragging = false;
        let isHorizontalSwipe = null;

        // Add swipe indicator elements if not present
        if (!item.querySelector('.swipe-indicator-left')) {
            const leftIndicator = document.createElement('span');
            leftIndicator.className = 'swipe-indicator swipe-indicator-left';
            leftIndicator.textContent = '✓';
            item.appendChild(leftIndicator);

            const rightIndicator = document.createElement('span');
            rightIndicator.className = 'swipe-indicator swipe-indicator-right';
            rightIndicator.textContent = '📅';
            item.appendChild(rightIndicator);
        }

        item.addEventListener('touchstart', (e) => {
            if (e.touches.length > 1) return;
            startX = e.touches[0].clientX;
            startY = e.touches[0].clientY;
            isDragging = true;
            isHorizontalSwipe = null;
            item.style.transition = 'none';
        }, { passive: true });

        item.addEventListener('touchmove', (e) => {
            if (!isDragging) return;

            currentX = e.touches[0].clientX - startX;
            const currentY = e.touches[0].clientY - startY;

            // Determine swipe direction on first significant move
            if (isHorizontalSwipe === null && (Math.abs(currentX) > 10 || Math.abs(currentY) > 10)) {
                isHorizontalSwipe = Math.abs(currentX) > Math.abs(currentY);
            }

            if (!isHorizontalSwipe) return;

            // Prevent vertical scrolling during horizontal swipe
            e.preventDefault();

            // Clamp movement
            const clampedX = Math.max(-SWIPE_MAX, Math.min(SWIPE_MAX, currentX));

            // Apply transform
            item.style.transform = `translateX(${clampedX}px)`;

            // Add visual feedback classes
            item.classList.toggle('swiping-right', currentX > SWIPE_THRESHOLD / 2);
            item.classList.toggle('swiping-left', currentX < -SWIPE_THRESHOLD / 2);
        }, { passive: false });

        item.addEventListener('touchend', () => {
            if (!isDragging) return;
            isDragging = false;

            item.style.transition = 'transform 0.2s ease';
            item.style.transform = '';
            item.classList.remove('swiping-right', 'swiping-left');

            if (isHorizontalSwipe) {
                const lineIndex = item.dataset.lineIndex;

                if (currentX > SWIPE_THRESHOLD) {
                    // Swipe right: Mark as done
                    if (lineIndex) {
                        item.classList.add('haptic-feedback');
                        toggleTodo({ stopPropagation: () => {} }, lineIndex);
                    }
                } else if (currentX < -SWIPE_THRESHOLD) {
                    // Swipe left: Postpone to tomorrow
                    if (lineIndex) {
                        item.classList.add('haptic-feedback');
                        postponeTodo({ stopPropagation: () => {} }, lineIndex, 'tomorrow');
                    }
                }
            }

            currentX = 0;
            isHorizontalSwipe = null;
        }, { passive: true });

        item.addEventListener('touchcancel', () => {
            isDragging = false;
            item.style.transition = 'transform 0.2s ease';
            item.style.transform = '';
            item.classList.remove('swiping-right', 'swiping-left');
            currentX = 0;
            isHorizontalSwipe = null;
        }, { passive: true });
    });
}
