/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./src/**/*.rs", "./index.html"],
  plugins: [require("daisyui")],
  daisyui: {
    themes: ["light", "dark"],
  },
};
