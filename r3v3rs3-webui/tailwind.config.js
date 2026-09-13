module.exports = {
    mode: "jit",
    content: {
      files: ["src/**/*.rs", "**/*.html"],
    },
    // The head script of index.html sets the dark class from the r3v3rs3_theme cookie.
    darkMode: "class",
    theme: {
      extend: {},
    },
    variants: {
      extend: {},
    },
    plugins: [],
  };