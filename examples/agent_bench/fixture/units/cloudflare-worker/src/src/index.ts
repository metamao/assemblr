export default {
  async fetch(request: Request): Promise<Response> {
    return Response.json({ ok: true });
  },
};
