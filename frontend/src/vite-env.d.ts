/// <reference types="vite/client" />

// TypeScript 6 enforces stricter side-effect import checking — without
// these declarations, `import './foo.css'` reports TS2882 "Cannot find
// module or type declarations for side-effect import". Vite handles the
// actual transformation at build time, so the ambient module shape is
// just a way to acknowledge the import to the type checker.
declare module '*.css';
declare module '*.scss';
declare module '*.svg' {
  const content: string;
  export default content;
}
declare module '*.png' {
  const content: string;
  export default content;
}
declare module '*.jpg' {
  const content: string;
  export default content;
}
declare module '*.gif' {
  const content: string;
  export default content;
}
