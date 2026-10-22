module.exports = {
    mode: "jit",
    content: {
      files: ["src/**/*.rs", "**/*.html"],
    },
    // The head script of index.html sets the dark class from the r3v3rs3_theme cookie.
    darkMode: "class",
    theme: {
      extend: {
        colors: {
          primary: {
            50: "#eff6ff",
            100: "#dbeafe",
            200: "#bfdbfe",
            300: "#93c5fd",
            400: "#60a5fa",
            500: "#3b82f6",
            600: "#2563eb",
            700: "#1d4ed8",
            800: "#1e40af",
            900: "#1e3a8a",
            950: "#172554",
          },
          neutral: {
            750: "#333333",
            850: "#1f1f1f",
          },
        },
      },
    },
    variants: {
      extend: {},
    },
    plugins: [],
  };