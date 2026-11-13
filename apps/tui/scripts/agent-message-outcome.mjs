/** Real-product TTY assertion, called after a keyboard/render barrier on the
 * parent session once a controlled child has delivered its completion. */
export function assertAgentMessageDisplay(text, resultMarker) {
  if (!text.includes("Agent message") || !text.includes(resultMarker)) {
    throw new Error("Child completion must display its Agent message and result body");
  }
  if (/\[system\]\s+Child agent/.test(text)) {
    throw new Error("Agent message still displays the generated child preamble");
  }
}
