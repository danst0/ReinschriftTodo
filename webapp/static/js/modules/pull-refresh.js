/**
 * Pull-to-refresh functionality.
 */

let pullStartY = 0;
let pullMoveY = 0;
let isPulling = false;
let canPull = true;
const pullThreshold = 80;
let pullToRefresh = null;
let onRefresh = null;

/**
 * Initialize pull-to-refresh.
 * @param {function} refreshFn - Function to call on refresh
 */
export function initPullToRefresh(refreshFn) {
    pullToRefresh = document.getElementById('pull-to-refresh');
    onRefresh = refreshFn;

    if (!pullToRefresh) return;

    document.addEventListener('touchstart', handleTouchStart, { passive: true });
    document.addEventListener('touchmove', handleTouchMove, { passive: true });
    document.addEventListener('touchend', handleTouchEnd, { passive: true });
}

function handleTouchStart(e) {
    if (window.scrollY === 0 && canPull) {
        pullStartY = e.touches[0].clientY;
        isPulling = true;
    }
}

function handleTouchMove(e) {
    if (!isPulling || window.scrollY > 0) {
        isPulling = false;
        return;
    }

    pullMoveY = e.touches[0].clientY - pullStartY;

    if (pullMoveY > 0 && pullToRefresh) {
        // Apply resistance
        const resistance = Math.min(pullMoveY / 2.5, pullThreshold);
        pullToRefresh.style.transform = `translateY(${resistance - 60}px)`;

        if (pullMoveY > pullThreshold) {
            pullToRefresh.classList.add('visible');
        }
    }
}

function handleTouchEnd() {
    if (!isPulling || !pullToRefresh) return;

    if (pullMoveY > pullThreshold) {
        // Trigger refresh
        pullToRefresh.classList.add('refreshing');
        canPull = false;

        if (onRefresh) {
            Promise.resolve(onRefresh()).then(() => {
                setTimeout(() => {
                    pullToRefresh.classList.remove('visible', 'refreshing');
                    pullToRefresh.style.transform = '';
                    canPull = true;
                }, 500);
            });
        }
    } else {
        pullToRefresh.classList.remove('visible');
        pullToRefresh.style.transform = '';
    }

    isPulling = false;
    pullMoveY = 0;
}
