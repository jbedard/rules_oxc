function dec(_target, _key) {}
export class Service {
	@dec name = "svc";
	@dec describe(label) {
		return label ? label.length : 0;
	}
}
