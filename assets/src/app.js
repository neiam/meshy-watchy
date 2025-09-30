// Mesh Watchy Client-side JavaScript
// Simple theme management and UI enhancements

// Theme management
class ThemeManager {
    constructor() {
        this.currentTheme = localStorage.getItem('mesh-watchy-theme') || 'her';
        this.init();
    }

    init() {
        this.setTheme(this.currentTheme);
        this.setupThemeSelector();
    }

    setTheme(theme) {
        document.documentElement.setAttribute('data-theme', theme);
        localStorage.setItem('mesh-watchy-theme', theme);
        this.currentTheme = theme;
        
        // Dispatch theme change event
        window.dispatchEvent(new CustomEvent('themeChanged', { 
            detail: { theme } 
        }));
    }

    setupThemeSelector() {
        // Make setTheme available globally for onclick handlers
        window.setTheme = (theme) => this.setTheme(theme);
    }
}

// Notification system
class NotificationManager {
    constructor() {
        this.container = this.createContainer();
    }

    createContainer() {
        const container = document.createElement('div');
        container.className = 'fixed top-4 right-4 z-50 space-y-2';
        container.id = 'notifications';
        document.body.appendChild(container);
        return container;
    }

    show(message, type = 'info', duration = 5000) {
        const notification = document.createElement('div');
        const typeClasses = {
            success: 'alert-success',
            error: 'alert-error',
            warning: 'alert-warning',
            info: 'alert-info'
        };

        notification.className = `alert ${typeClasses[type]} shadow-lg max-w-sm`;
        notification.innerHTML = `
            <div class="flex items-center">
                <span>${message}</span>
                <button class="btn btn-ghost btn-xs ml-2" onclick="this.parentElement.parentElement.remove()">✕</button>
            </div>
        `;

        this.container.appendChild(notification);

        if (duration > 0) {
            setTimeout(() => {
                if (notification.parentNode) {
                    notification.remove();
                }
            }, duration);
        }

        return notification;
    }
}

// Initialize application
document.addEventListener('DOMContentLoaded', () => {
    console.log('Mesh Watchy initializing...');
    
    // Initialize managers
    window.themeManager = new ThemeManager();
    window.notifications = new NotificationManager();

    // Auto-refresh functionality
    const setupAutoRefresh = () => {
        const refreshInterval = 30000; // 30 seconds
        
        // Only refresh if not on a form page or modal is open
        const shouldRefresh = () => {
            const hasOpenModal = document.querySelector('.modal.modal-open');
            const hasActiveForm = document.querySelector('form:focus-within');
            return !hasOpenModal && !hasActiveForm;
        };

        setInterval(() => {
            if (shouldRefresh() && document.visibilityState === 'visible') {
                // Use HTMX to refresh specific parts of the page
                const refreshElements = document.querySelectorAll('[data-auto-refresh]');
                refreshElements.forEach(element => {
                    if (element.hasAttribute('hx-get') && typeof htmx !== 'undefined') {
                        htmx.trigger(element, 'refresh');
                    }
                });
            }
        }, refreshInterval);
    };

    // Initialize auto-refresh if enabled
    if (!document.querySelector('[data-no-auto-refresh]')) {
        setupAutoRefresh();
    }

    // Setup keyboard shortcuts
    document.addEventListener('keydown', (event) => {
        // Ctrl/Cmd + K for quick search (if implemented)
        if ((event.ctrlKey || event.metaKey) && event.key === 'k') {
            event.preventDefault();
            const searchInput = document.querySelector('input[type="search"], input[placeholder*="search" i]');
            if (searchInput) {
                searchInput.focus();
            }
        }

        // Escape to close modals
        if (event.key === 'Escape') {
            const openModals = document.querySelectorAll('.modal.modal-open');
            openModals.forEach(modal => {
                modal.classList.remove('modal-open');
            });
        }
    });

    console.log('Mesh Watchy frontend initialized');
});

// Utility functions for global use
window.MeshWatchy = {
    showNotification: (message, type = 'info') => {
        if (window.notifications) {
            window.notifications.show(message, type);
        }
    },
    
    setTheme: (theme) => {
        if (window.themeManager) {
            window.themeManager.setTheme(theme);
        }
    }
};
