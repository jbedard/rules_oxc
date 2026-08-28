export class Fields {
	declared;
	initialized = 1;
	static count;
	static label = "fields";
	constructor(value) {
		this.declared = value;
	}
}
