// Vite asset module declarations.
declare module "*.png" {
  const src: string;
  export default src;
}
declare module "*?url" {
  const src: string;
  export default src;
}
declare module "*.svg" {
  const src: string;
  export default src;
}
declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const cmp: DefineComponent<{}, {}, any>;
  export default cmp;
}
