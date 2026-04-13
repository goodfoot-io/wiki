/**
 * Local ambient module declaration for morphdom.
 *
 * morphdom's package uses `module.exports = morphdom` (CJS) with `export default`
 * in its `.d.ts`. tsgo resolves this as a non-callable module namespace. This
 * declaration overrides it with a callable default export that matches the
 * upstream signature.
 *
 * @summary Callable default export declaration for morphdom (tsgo CJS compat).
 */

declare module 'morphdom' {
  interface MorphDomOptions {
    getNodeKey?: (node: Node) => unknown;
    onBeforeNodeAdded?: (node: Node) => false | Node;
    onNodeAdded?: (node: Node) => void;
    onBeforeElUpdated?: (fromEl: HTMLElement, toEl: HTMLElement) => boolean;
    onElUpdated?: (el: HTMLElement) => void;
    onBeforeNodeDiscarded?: (node: Node) => boolean;
    onNodeDiscarded?: (node: Node) => void;
    onBeforeElChildrenUpdated?: (fromEl: HTMLElement, toEl: HTMLElement) => boolean;
    skipFromChildren?: (fromEl: HTMLElement) => boolean;
    addChild?: (parent: HTMLElement, child: HTMLElement) => void;
    childrenOnly?: boolean;
  }

  function morphdom(fromNode: Node, toNode: Node | string, options?: MorphDomOptions): Node;

  export default morphdom;
}
