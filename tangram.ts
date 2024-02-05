export default tg.target(async () => {
	await foo();
	await bar();
});

export let foo = tg.target(async () => {
	console.log("starting");
	for (let i = 0; i < 5; i++) {
		await tg.sleep(1);
		console.log(`print ${i + 1}`);
	}
	return "Hello, World!";
});

export let bar = tg.target(async () => {
	console.log("starting");
	for (let i = 0; i < 5; i++) {
		await tg.sleep(1);
		console.log(`print ${i + 1}`);
	}
	return "Hello, World!";
});
