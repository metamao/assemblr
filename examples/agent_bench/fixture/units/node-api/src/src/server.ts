import Fastify from "fastify";
const app = Fastify();
app.get("/", async () => ({ ok: true }));
app.listen({ port: 3000 });
