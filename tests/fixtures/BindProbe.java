/* Test-only real JDK classic/NIO wildcard-listener probe for the Nacos fixture adapter. */
import java.net.*;
import java.nio.channels.ServerSocketChannel;
class BindProbe {
    public static void main(String[] args) throws Exception {
        try (ServerSocket socket=new ServerSocket(0)) {
            System.out.println("PORT="+socket.getLocalPort());System.out.flush();System.in.read();
            try (Socket client=new Socket(InetAddress.getLoopbackAddress(),socket.getLocalPort());Socket accepted=socket.accept()) {
                accepted.getOutputStream().write(7);
                if (client.getInputStream().read()!=7) throw new AssertionError("Classic listener failed");
            }
        }
        try (ServerSocketChannel channel=ServerSocketChannel.open()) {
            channel.bind(new InetSocketAddress(0));
            InetSocketAddress address=(InetSocketAddress)channel.getLocalAddress();
            System.out.println("PORT="+address.getPort());System.out.flush();System.in.read();
            try (Socket client=new Socket(InetAddress.getLoopbackAddress(),address.getPort());Socket accepted=channel.accept().socket()) {
                accepted.getOutputStream().write(9);
                if (client.getInputStream().read()!=9) throw new AssertionError("NIO listener failed");
            }
        }
        System.out.println("Actual JDK classic/NIO listeners: loopback bound and local exchange passed");
    }
}
