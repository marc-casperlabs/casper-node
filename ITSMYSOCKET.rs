
/// Create an unused, bound but rebindable socket on localhost.
///
/// Asks the OS to bind an unused socket, but enables rebinding via `SO_REUSEADDR`. This essentially
/// prevents allocating the same port twice, as long as the `TcpListener` is kept around.
pub(crate) fn unused_socket_on_localhost() -> (u16, TcpListener) {
  let inner = Socket::new_raw(libc::AF_UNIX, libc::SOCK_STREAM)?;
  let (addr, len) = sockaddr_un(path.as_ref())?;

  cvt(libc::bind(*inner.as_inner(), &addr as *const _ as *const _, len as _));
  cvt(libc::listen(*inner.as_inner(), 128));

  Ok(UnixListener(inner))



  let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0))
      .expect("could not bind new random port on localhost");
  let local_addr = listener
      .local_addr()
      .expect("local listener has no address?");

  // Make the port reusable.
  socket::setsockopt(listener.as_raw_fd(), ReusePort, &true)
      .expect("could not set SO_REUSEADDR on port");

  info!(%local_addr, "OS generated random reusable socket on localhost");

  (local_addr.port(), listener)
}
