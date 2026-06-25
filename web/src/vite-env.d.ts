/// <reference types="vite/client" />

// CSS imported with Vite's ?inline query resolves to the stylesheet text.
declare module "*.css?inline" {
  const css: string;
  export default css;
}
