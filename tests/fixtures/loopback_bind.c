/* Test-only adapter for server distributions whose auxiliary ports bind INADDR_ANY.
 * Load only into the owned fixture process; never into the desktop or shared services.
 */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <errno.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
static int (*next_bind)(int,const struct sockaddr*,socklen_t);
__attribute__((constructor)) static void init_bind(void){next_bind=dlsym(RTLD_NEXT,"bind");}
int bind(int fd,const struct sockaddr* address,socklen_t length){
 if(!next_bind){errno=ENOSYS;return -1;}
 if(!address||length>sizeof(struct sockaddr_storage)){errno=EINVAL;return -1;}
 struct sockaddr_storage local;memset(&local,0,sizeof(local));memcpy(&local,address,length);
 if(address->sa_family==AF_INET){
  if(length<sizeof(struct sockaddr_in)){errno=EINVAL;return -1;}
  struct sockaddr_in* a=(struct sockaddr_in*)&local;unsigned int host=ntohl(a->sin_addr.s_addr);
  if(host==INADDR_ANY)a->sin_addr.s_addr=htonl(INADDR_LOOPBACK);
  else if((host>>24)!=127){errno=EACCES;return -1;}
 }else if(address->sa_family==AF_INET6){
  if(length<sizeof(struct sockaddr_in6)){errno=EINVAL;return -1;}
  struct sockaddr_in6* a=(struct sockaddr_in6*)&local;
  if(IN6_IS_ADDR_UNSPECIFIED(&a->sin6_addr))a->sin6_addr=in6addr_loopback;
  else if(!IN6_IS_ADDR_LOOPBACK(&a->sin6_addr)&&!(IN6_IS_ADDR_V4MAPPED(&a->sin6_addr)&&a->sin6_addr.s6_addr[12]==127)){errno=EACCES;return -1;}
 }
 return next_bind(fd,(const struct sockaddr*)&local,length);
}
