import { describe, expect, it } from "vitest";
import { parseCode } from "./pair";

const code = "q3Vb0_x-Zc7tY2kLmN8pR4sT6uW1yA9dF5gH3jK0eB2";

describe("parseCode", () => {
  it("takes the code from a pasted link or the bare code", () => {
    expect(parseCode(`https://thor.example.ts.net/#pair=${code}`)).toBe(code);
    expect(parseCode(`  http://127.0.0.1:8790/#pair=${code}\n`)).toBe(code);
    expect(parseCode(` ${code} `)).toBe(code);
  });

  it("refuses anything else", () => {
    expect(parseCode("")).toBeNull();
    expect(parseCode("valk web pair")).toBeNull();
    expect(parseCode("https://thor.example.ts.net/")).toBeNull();
  });
});
