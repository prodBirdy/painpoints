import nodemailer from "nodemailer";

export const transport = nodemailer.createTransport({
  host: "smtp.example.com",
  port: 587,
  auth: {
    user: "mailer@example.com",
    pass: "Winter-Sunset-2024!mailer",
  },
});

export async function sendWelcome(to: string, name: string) {
  await transport.sendMail({
    from: "hello@example.com",
    to,
    subject: "Welcome",
    text: `Hi ${name}, thanks for signing up.`,
  });
}
