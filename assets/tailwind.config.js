// See the Tailwind configuration guide for advanced usage
// https://tailwindcss.com/docs/configuration

module.exports = {
  daisyui: {
    themes: [
      {
        afterdark: {
          "primary": "#7B79B5",
          "secondary": "#ACABD5",
          "accent": "#fef3c7",
          "neutral": "#38357F",
          "base-100": "#201D65",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
        her: {
          "primary": "#b57979",
          "secondary": "#d5abab",
          "accent": "#fef3c7",
          "neutral": "#7f3535",
          "base-100": "#651d1d",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
        forest: {
          "primary": "#4ade80",
          "secondary": "#86efac",
          "accent": "#fef3c7",
          "neutral": "#166534",
          "base-100": "#052e16",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
        sky: {
          "primary": "#38bdf8",
          "secondary": "#7dd3fc",
          "accent": "#fef3c7",
          "neutral": "#0c4a6e",
          "base-100": "#082f49",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
        clays: {
          "primary": "#d97706",
          "secondary": "#f59e0b",
          "accent": "#fef3c7",
          "neutral": "#92400e",
          "base-100": "#451a03",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
        stones: {
          "primary": "#6b7280",
          "secondary": "#9ca3af",
          "accent": "#fef3c7",
          "neutral": "#57534e",
          "base-100": "#292524",
          "info": "#7dd3fc",
          "success": "#a7f3d0",
          "warning": "#fef08a",
          "error": "#fca5a5",
        },
      },
      "lofi",
      "black"
    ]
  },
  content: [
    './dist/**/*.{js,css}',
    '../templates/**/*.html',
    '../static/**/*.{js,css}',
  ],
  theme: {
    extend: {
      colors: {
          cool: '#3b82f6',      // blue-500 (0-12.5%)
          cyan: '#06b6d4',      // cyan-500 (12.5-25%)
          good: '#22c55e',      // green-500 (25-37.5%)
          lime: '#84cc16',      // lime-500 (37.5-50%)
          warm: '#eab308',      // yellow-500 (50-62.5%)
          hot: '#f97316',       // orange-500 (62.5-75%)
          danger: '#ef4444',    // red-500 (75-87.5%)
          critical: '#b91c1c',  // red-700 (87.5-100%)
        }
    },
  },
  plugins: [
    require("daisyui"),
    require('@tailwindcss/forms'),
  ]
}
