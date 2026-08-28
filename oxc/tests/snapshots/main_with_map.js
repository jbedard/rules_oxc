export const greeting = "hello";
export function add(a, b) {
	return a + b;
}
export class Vec {
	x;
	y;
	constructor(x, y) {
		this.x = x;
		this.y = y;
	}
	length() {
		return Math.sqrt(this.x * this.x + this.y * this.y);
	}
}

//# sourceMappingURL=main.js.map