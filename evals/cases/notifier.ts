export interface NotificationChannel {
  send(userId: string, message: string): Promise<void>;
}

export interface NotificationChannelFactory {
  create(kind: string): NotificationChannel;
}

class EmailChannel implements NotificationChannel {
  constructor(private readonly mailer: { mail(to: string, body: string): Promise<void> }) {}
  async send(userId: string, message: string) {
    await this.mailer.mail(userId, message);
  }
}

export class DefaultNotificationChannelFactory implements NotificationChannelFactory {
  constructor(private readonly mailer: { mail(to: string, body: string): Promise<void> }) {}
  create(kind: string): NotificationChannel {
    switch (kind) {
      case "email":
        return new EmailChannel(this.mailer);
      default:
        return new EmailChannel(this.mailer);
    }
  }
}

export class NotificationService {
  constructor(private readonly factory: NotificationChannelFactory) {}
  async notify(userId: string, message: string) {
    const channel = this.factory.create("email");
    await channel.send(userId, message);
  }
}
