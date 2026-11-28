import { describe, expect, it } from "vitest";
import { clampSize, sizeFromDrag, sizeFromKey } from "./resize";

const bounds = { min: 120, max: 600 };

describe("clampSize", () => {
  it("keeps a size inside its bounds", () => {
    expect(clampSize(300, bounds)).toBe(300);
    expect(clampSize(10, bounds)).toBe(120);
    expect(clampSize(900, bounds)).toBe(600);
  });

  it("prefers the minimum when the bounds are inverted, as in a very small window", () => {
    expect(clampSize(300, { min: 120, max: 80 })).toBe(120);
  });
});

describe("sizeFromDrag", () => {
  it("grows a bottom pane when its top handle is dragged up", () => {
    // Pointer starts at y=500 and moves to y=400: 100px up.
    expect(sizeFromDrag(288, 500, 400, bounds)).toBe(388);
  });

  it("shrinks a bottom pane when its top handle is dragged down", () => {
    expect(sizeFromDrag(288, 500, 560, bounds)).toBe(228);
  });

  it("grows a right-hand pane when its left handle is dragged left", () => {
    expect(sizeFromDrag(384, 700, 620, bounds)).toBe(464);
  });

  it("stops at the bounds however far the pointer travels", () => {
    expect(sizeFromDrag(288, 500, -2000, bounds)).toBe(600);
    expect(sizeFromDrag(288, 500, 2000, bounds)).toBe(120);
  });
});

describe("sizeFromKey", () => {
  it("resizes the height with the up and down arrows", () => {
    expect(sizeFromKey(288, "ArrowUp", "y", 24, bounds)).toBe(312);
    expect(sizeFromKey(288, "ArrowDown", "y", 24, bounds)).toBe(264);
  });

  it("resizes the run-log width with the left and right arrows", () => {
    expect(sizeFromKey(384, "ArrowLeft", "x", 24, bounds)).toBe(408);
    expect(sizeFromKey(384, "ArrowRight", "x", 24, bounds)).toBe(360);
  });

  it("ignores keys that do not resize on that axis", () => {
    expect(sizeFromKey(288, "ArrowLeft", "y", 24, bounds)).toBeNull();
    expect(sizeFromKey(384, "a", "x", 24, bounds)).toBeNull();
  });

  it("stops at the bounds", () => {
    expect(sizeFromKey(595, "ArrowUp", "y", 24, bounds)).toBe(600);
    expect(sizeFromKey(125, "ArrowDown", "y", 24, bounds)).toBe(120);
  });
});

describe("growsToward end", () => {
  it("grows a left-hand pane when its right handle is dragged right", () => {
    // The changes list: its handle is on the right edge, so dragging right widens it.
    expect(sizeFromDrag(288, 300, 380, bounds, "end")).toBe(368);
  });

  it("shrinks a left-hand pane when its right handle is dragged left", () => {
    expect(sizeFromDrag(288, 300, 240, bounds, "end")).toBe(228);
  });

  it("grows a top pane when its bottom handle is dragged down", () => {
    expect(sizeFromDrag(288, 500, 560, bounds, "end")).toBe(348);
  });

  it("maps arrow keys to the direction that grows the pane", () => {
    expect(sizeFromKey(288, "ArrowRight", "x", 24, bounds, "end")).toBe(312);
    expect(sizeFromKey(288, "ArrowLeft", "x", 24, bounds, "end")).toBe(264);
    expect(sizeFromKey(288, "ArrowLeft", "x", 24, bounds)).toBe(312);
  });
});
